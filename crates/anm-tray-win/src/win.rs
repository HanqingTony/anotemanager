//! Windows 外壳：托盘 + 全局快捷键 + 全屏覆盖层（纯 Win32 + windows-sys，无 GUI 框架）。
//!
//! 职责边界（与 anm-tray-core 的分工）：
//! - **核心**（`anm_tray_core`）：状态模型、卡片布局/命中/拖动/滚动、命令解析、
//!   编辑器读写、IPC——纯逻辑，跨平台复用；
//! - **本外壳**：窗口与消息循环、逐像素渲染、输入事件 → 调核心 → 执行
//!   [`Action`]、平台路径转换（wslpath）、托盘/热键。
//!
//! 线程模型：**单线程消息循环**，全部窗口跑在同一线程。
//! 安全规则（吃过两次亏）：**持有 `SHELL` 借用期间绝不调用会同步重入
//! 窗口过程的 API**（`SetWindowTextW` / `GetWindowTextW` / `SendMessageW` /
//! `SetCapture` / `ReleaseCapture` 等）——先取出数据、释放借用、再调用。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows_sys::core::{w, PCWSTR};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE,
    WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateRoundRectRgn,
    CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, FillRgn, FrameRect,
    InvalidateRect, RoundRect, SelectObject, SetBkColor, SetBkMode, SetTextColor, SetWindowRgn, AC_SRC_ALPHA, AC_SRC_OVER,
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
    DT_SINGLELINE, DT_TOP, DT_VCENTER, DT_WORDBREAK, FF_DONTCARE, HBRUSH, HDC, HFONT, HGDIOBJ,
    OUT_DEFAULT_PRECIS, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::{EM_SETSEL, WM_CTLCOLOR, WM_MOUSELEAVE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent,
    UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, TME_LEAVE, TRACKMOUSEEVENT,
    VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, ShellExecuteExW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD,
    NIM_DELETE, NOTIFYICONDATAW, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallWindowProcW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    GetWindowTextLengthW, GetWindowTextW, LoadIconW, MessageBoxW, PostMessageW, PostQuitMessage,
    GetClientRect, LoadCursorW, RegisterClassW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, TrackPopupMenu, TranslateMessage, ULW_ALPHA,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_WANTRETURN, GWLP_WNDPROC, HWND_TOPMOST, IDC_ARROW, MA_NOACTIVATE, MB_ICONERROR,
    MF_STRING, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW, SW_SHOWNORMAL,
    KillTimer, SetTimer, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CAPTURECHANGED, WM_CREATE, WM_CTLCOLOREDIT,
    WM_DESTROY, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEWHEEL, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETFONT,
    WM_SYSKEYDOWN, WM_TIMER, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, WS_VSCROLL, WNDCLASSW,
};

use anm_core::protocol::Request;
use anm_core::query::DirOverview;
use anm_tray_core::cards::{self, CardRow, Hit, LayoutParams, Rect};
use anm_tray_core::commands;
use anm_tray_core::hotkey::{self, Hotkey};
use anm_tray_core::ipc;
use anm_tray_core::model::{self, Action, DragState, TrayState};

use crate::wslpath;

/// 托盘回调消息（Shell_NotifyIcon 通过 lParam 携带鼠标事件类型）。
const TRAY_MSG: u32 = WM_APP + 1;
/// 异步拉取总览完成的消息（后台线程 PostMessage 到主窗口）。
const MSG_OVERVIEW_DONE: u32 = WM_APP + 2;

/// 后台拉取的主界面数据：目录总览 + skatch 卡片数据（路径 / 根目录 / 段落）。
struct HomeData {
    /// 一级目录总览
    dirs: Vec<DirOverview>,
    /// skatch 信息（path, root, segments）；拉取失败为 None（不阻塞主界面）
    skatch: Option<(String, String, Vec<String>)>,
}

/// 后台拉取总览的共享结果槽（一次一个；主线程消费后清空）。
static HOME_RESULT: Mutex<Option<Result<HomeData, String>>> = Mutex::new(None);
/// 是否已有一次拉取在进行（防重复）。
static OVERVIEW_FETCHING: AtomicBool = AtomicBool::new(false);
/// 全局快捷键注册 id。
const HOTKEY_ID: i32 = 1;
/// 默认全局快捷键（未配置时用；字符串形式与 hotkey::parse 对应）。
const DEFAULT_HOTKEY: &str = "Alt+Shift+Z";
/// 托盘右键菜单：显示（原「激活」，改名见需求）。
const MENU_ACTIVATE: usize = 1;
/// 托盘右键菜单：设置服务地址。
const MENU_SETTINGS: usize = 2;
/// 托盘右键菜单：退出。
const MENU_EXIT: usize = 3;
/// 托盘右键菜单：设置快捷键。
const MENU_HOTKEY: usize = 4;
/// 隐藏主窗口类名。
const TRAY_CLASS: &str = "AnmTrayWin";
/// 变暗覆盖层窗口类名。
const OVERLAY_CLASS: &str = "AnmOverlayWin";
/// 输入框/编辑器窗口类名。
const INPUT_CLASS: &str = "AnmInputWin";
/// 设置对话框窗口类名。
const SETTINGS_CLASS: &str = "AnmSettingsWin";
/// 快捷键设置对话框窗口类名。
const HOTKEY_CLASS: &str = "AnmHotkeyWin";
/// 轻提示（toast）窗口类名。
const TOAST_CLASS: &str = "AnmToastWin";
/// 嵌入的应用图标资源 id（见 assets/anm.rc：`1 ICON "anm.ico"`）。
const APP_ICON_ID: usize = 1;
/// 单例互斥体名称（会话内唯一；第二个实例启动时直接退出）。
const SINGLETON_MUTEX: &str = "Local\\anm-tray-win";
/// 变暗层背景 alpha（逐像素合成时纯黑像素的透明度；0=全透明，255=不透明）。
const DIM_ALPHA: u8 = 160;
/// 点击与拖动的判定阈值（像素）：累计位移小于该值视为点击，否则视为拖动。
const DRAG_THRESHOLD: i32 = 4;
/// 卡片位置记忆文件（%APPDATA%/anm-tray-win/layout.json）。
const LAYOUT_DIR: &str = "anm-tray-win";
const LAYOUT_FILE: &str = "layout.json";
/// 编辑器模式：窗口尺寸与布局常量。
const EDITOR_W: i32 = 760;
const EDITOR_H: i32 = 440;
/// 编辑器头部信息栏高度（文件名 + 路径）
const EDITOR_HEADER_H: i32 = 42;
/// 自绘按钮高度
const EDITOR_BTN_H: i32 = 26;
/// 自绘按钮标识（命中测试用）
const BTN_LOCATION: u8 = 1;
const BTN_SAVE: u8 = 2;
const BTN_CLOSE: u8 = 3;
/// 设置对话框尺寸。
const SETTINGS_W: i32 = 420;
const SETTINGS_H: i32 = 252;

/// 快捷键设置对话框尺寸。
const HOTKEY_W: i32 = 420;
const HOTKEY_H: i32 = 180;

/// 轻提示（toast）尺寸与自动隐藏毫秒。
const TOAST_W: i32 = 420;
const TOAST_H: i32 = 56;
const TOAST_TIMER: usize = 1;
const TOAST_MS: usize = 3200;
/// 单击时间阈值（毫秒）：按住文件行超过该时长后松开 = 预览结束（关闭编辑器）；
/// 未超过 = 正常单击（编辑器保持打开）。
const CLICK_MS: u128 = 300;
/// EM_SCROLLCARET（滚动到光标处；windows-sys 未导出，按文档值使用）
const EM_SCROLLCARET: u32 = 0x00B7;
/// EM_CHARFROMPOS（客户区坐标 → 字符下标；EDIT/RichEdit 均支持）
const EM_CHARFROMPOS: u32 = 0x00D7;
/// RichEdit 段落格式（EM_SETPARAFORMAT 0x0445；PARAFORMAT2 需手动定义）
const EM_SETPARAFORMAT: u32 = 0x0445;
/// RichEdit 背景色（EM_SETBKGNDCOLOR 0x0443；wParam=0 使用 lParam 颜色）
const EM_SETBKGNDCOLOR: u32 = 0x0443;
/// RichEdit 字符格式（EM_SETCHARFORMAT 0x0444；SCF_ALL=4 应用于全文）
const EM_SETCHARFORMAT: u32 = 0x0444;
const SCF_ALL: usize = 0x0004;
const CFM_COLOR: u32 = 0x4000_0000;
/// CHARFORMAT2W 字体名数组长度（LF_FACESIZE）
const LF_FACESIZE: usize = 32;
const PFM_SPACEBEFORE: u32 = 0x0000_0040;
const PFM_SPACEAFTER: u32 = 0x0000_0080;
/// RichEdit 2.0 控件类（msftedit.dll）
const RICHEDIT_CLASS: &str = "RICHEDIT50W";
/// 设置对话框按钮标识。
const BTN_SETTINGS_OK: u8 = 1;
const BTN_SETTINGS_CANCEL: u8 = 2;
/// 视为文本、用内置编辑器打开的文件扩展名。
const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "txt", "text", "log", "json", "toml", "yaml", "yml", "csv", "ini", "conf",
    "cfg", "py", "rs", "sh", "ts", "js", "html", "css",
];

/// 内容行（文件/子目录）按下状态：编辑器预览与拖动移动共用。
struct RowPress {
    /// 所在卡片下标
    card: usize,
    /// 可见行下标
    row: usize,
    /// 按下位置（判定拖动阈值）
    start_x: i32,
    /// 按下位置
    start_y: i32,
    /// 已移动超过阈值（进入拖动）
    moved: bool,
    /// 按下时刻（区分单击与长按预览）
    press_at: std::time::Instant,
    /// 源文件 core 路径（`File` 行才有；子目录行为 None）
    path: Option<String>,
    /// 源行是否为 skatch 段落（段落下标 = 真实行 - 1；拖到目录卡 = 抽取成文件）
    skatch_index: Option<usize>,
    /// 按下时编辑器是否已打开同一文件（松开快速单击时 toggle 关闭用）
    was_open: bool,
    /// 按下时预览打开/关闭的子卡片目录（子目录行按下即显示子卡预览）
    subcard_dir: Option<String>,
    /// 按下时是否打开了子卡预览（长按/拖动松开时关闭预览）
    subcard_opened: bool,
}

/// RichEdit 字符格式结构（CHARFORMAT2W，仅用到颜色字段）。
#[repr(C)]
struct Charfmt2 {
    cb_size: u32,
    dw_mask: u32,
    dw_effects: u32,
    y_height: i32,
    y_offset: i32,
    cr_text_color: u32,
    b_charset: u8,
    b_pitch_and_family: u8,
    sz_face_name: [u16; LF_FACESIZE],
    w_weight: u16,
    s_spacing: i16,
    cr_back_color: u32,
    lcid: u32,
    dw_reserved: u32,
    s_style: i16,
    w_kerning: u16,
    b_underline_type: u8,
    b_animation: u8,
    b_rev_author: u8,
    b_underline_color: u8,
}

/// RichEdit 全文文字设为白色（EM_SETCHARFORMAT SCF_ALL；须在内容加载后调用）。
fn set_rich_text_white(edit: HWND) {
    unsafe {
        let mut cf: Charfmt2 = std::mem::zeroed();
        cf.cb_size = size_of::<Charfmt2>() as u32;
        cf.dw_mask = CFM_COLOR;
        cf.dw_effects = 0; // 不用自动色，强制指定
        cf.cr_text_color = rgb(235, 238, 245);
        SendMessageW(
            edit,
            EM_SETCHARFORMAT,
            SCF_ALL as usize,
            &cf as *const Charfmt2 as isize,
        );
    }
}

/// RichEdit 段落格式结构（PARAFORMAT2，仅用到间距字段）。
#[repr(C)]
struct Parafmt2 {
    cb_size: u32,
    dw_mask: u32,
    w_numbering: u16,
    w_reserved: u16,
    dx_start_indent: i32,
    dx_right_indent: i32,
    dx_offset: i32,
    w_alignment: u16,
    c_tab_count: i16,
    rgx_tabs: [i32; 32],
    dy_space_before: i32,
    dy_space_after: i32,
    dy_line_spacing: i32,
    s_style: i16,
    b_line_spacing_rule: u8,
    b_outline_level: u8,
    w_shading_weight: u16,
    w_shading_style: u16,
    w_numbering_start: u16,
    w_numbering_style: u16,
    w_numbering_tab: u16,
    w_border_space: u16,
    w_border_width: u16,
    w_border_styles: u16,
}

/// 外壳状态：窗口句柄 + 共享核心状态。
struct ShellState {
    /// 隐藏主窗口（托盘图标宿主 + 全局热键注册窗口）句柄
    tray_hwnd: HWND,
    /// 变暗覆盖层（全屏、半透明）窗口句柄
    hwnd: HWND,
    /// 输入框/编辑器窗口句柄（不透明）
    input_hwnd: HWND,
    /// 输入框（EDIT 子控件）句柄——launcher 单行
    edit: HWND,
    /// 编辑器多行 EDIT（换行/滚动由创建样式决定，运行时切换样式不生效）
    edit_editor: HWND,
    /// 编辑器头部文件名输入框（单行 EDIT，可改名；Enter=保存退出）
    rename_edit: HWND,
    /// 文件名输入框的原窗口过程
    old_rename_edit_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    /// 编辑器 EDIT 的原窗口过程
    old_edit_editor_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    /// 深色主题画刷（输入框/编辑器背景，创建一次复用）
    dark_brush: HBRUSH,
    /// 编辑器自绘按钮的悬停项（0=无，1=打开所在位置，2=保存，3=✕）
    btn_hover: u8,
    /// 设置对话框窗口 / 地址输入框 / 令牌输入框 / 原窗口过程 / 按钮悬停 / 错误提示
    settings_hwnd: HWND,
    settings_edit: HWND,
    settings_token_edit: HWND,
    old_settings_edit_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    old_settings_token_edit_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    settings_btn_hover: u8,
    settings_error: String,
    /// 快捷键设置对话框 / 中央提示 / 待确认组合
    hotkey_hwnd: HWND,
    hotkey_hint: String,
    hotkey_pending: Option<Hotkey>,
    /// 轻提示（toast）窗口与文本
    toast_hwnd: HWND,
    toast_text: String,
    /// EDIT 原窗口过程（子类化后用于转发未处理的按键）
    old_edit_proc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
    /// 卡片/结果字体
    card_font: HFONT,
    /// 输入框字体
    input_font: HFONT,
    /// 布局参数（与核心共享）
    params: LayoutParams,
    /// 跨平台共享状态（卡片、输入框矩形、位置记忆、编辑器……）
    core: TrayState,
    /// 当前已注册成功的全局快捷键（None = 未注册）
    hotkey: Option<Hotkey>,
    /// 覆盖层当前是否可见（热键切换「显示/隐藏」用）
    overlay_visible: bool,
    /// 新建笔记模式：Some(目录) = 输入框处于"输入文件名回车创建"状态
    pending_new_note: Option<String>,
    /// 内容行按下状态（文件行：编辑器预览/拖动移动；子目录行：仅点击）
    row_press: Option<RowPress>,
    /// skatch 导航条 hover 段落下标（联动覆盖层 skatch 卡片指示条）
    nav_hover: Option<usize>,
}

thread_local! {
    /// 外壳状态（单线程 UI，进程内唯一，WndProc 经 thread_local 访问）。
    static SHELL: RefCell<ShellState> = RefCell::new(ShellState::placeholder());
}

// ---------------------------------------------------------------------------
// 入口与窗口注册
// ---------------------------------------------------------------------------

/// 程序入口（Windows）：注册窗口类、创建隐藏主窗 + 变暗层 + 输入窗（含 EDIT
/// 与编辑器按钮），然后进入消息循环直到「退出」。
pub fn run() -> ! {
    unsafe {
        let hinstance: HINSTANCE = GetModuleHandleW(null_mut());

        // 单例：命名互斥体已存在说明已有实例在跑，直接退出
        let mutex_name = to_wide(SINGLETON_MUTEX);
        CreateMutexW(null_mut(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            std::process::exit(0);
        }

        // 注册窗口类
        let tray_class = to_wide(TRAY_CLASS);
        let overlay_class = to_wide(OVERLAY_CLASS);
        let input_class = to_wide(INPUT_CLASS);
        let settings_class = to_wide(SETTINGS_CLASS);
        let hotkey_class = to_wide(HOTKEY_CLASS);
        let toast_class = to_wide(TOAST_CLASS);
        register_class(hinstance, tray_class.as_ptr(), Some(tray_wndproc));
        register_class(hinstance, overlay_class.as_ptr(), Some(overlay_wndproc));
        register_class(hinstance, input_class.as_ptr(), Some(input_wndproc));
        register_class(hinstance, settings_class.as_ptr(), Some(settings_wndproc));
        register_class(hinstance, hotkey_class.as_ptr(), Some(hotkey_wndproc));
        register_class(hinstance, toast_class.as_ptr(), Some(toast_wndproc));

        // 隐藏主窗口：WM_CREATE 中注册热键 + 托盘图标
        let tray_class = to_wide(TRAY_CLASS);
        let tray_hwnd = CreateWindowExW(
            0,
            tray_class.as_ptr(),
            w!("anm-tray-win"),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if tray_hwnd.is_null() {
            fatal("创建主窗口失败");
        }

        // 变暗覆盖层：全屏置顶 + 分层（逐像素合成），WS_EX_NOACTIVATE 不抢输入框焦点
        let overlay_class = to_wide(OVERLAY_CLASS);
        let overlay = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            overlay_class.as_ptr(),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if overlay.is_null() {
            fatal("创建覆盖层窗口失败");
        }

        // 输入框窗口：独立小窗口（不透明，与变暗层分离保证文字清晰）
        let input_class = to_wide(INPUT_CLASS);
        let input_hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            input_class.as_ptr(),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if input_hwnd.is_null() {
            fatal("创建输入框窗口失败");
        }

        // 启动时注入持久化的托盘配置：服务地址 / 访问令牌 / 全局快捷键
        let cfg0 = load_tray_config();
        if let Some(addr) = cfg0.server_addr.clone() {
            ipc::set_server_addr_override(Some(addr));
        }
        if let Some(token) = cfg0.server_token.clone() {
            ipc::set_server_token_override(Some(token));
        }
        let startup_hotkey = cfg0
            .hotkey
            .as_deref()
            .and_then(hotkey::parse)
            .or_else(|| hotkey::parse(DEFAULT_HOTKEY));

        let (edit, input_font) = SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            sh.tray_hwnd = tray_hwnd;
            sh.hwnd = overlay;
            sh.input_hwnd = input_hwnd;
            sh.hotkey = startup_hotkey;
            sh.card_font = create_font(15);
            sh.input_font = create_font(17);
            sh.core = TrayState::new(load_positions());
            sh.params = LayoutParams::default();

            // 单行输入框（EDIT）
            let edit = CreateWindowExW(
                0,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                input_hwnd,
                null_mut(),
                hinstance,
                null_mut(),
            );
            let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_wndproc as *const () as isize);
            sh.old_edit_proc = Some(std::mem::transmute::<isize, _>(old));
            sh.edit = edit;

            // 编辑器多行编辑控件：RichEdit（msftedit.dll 的 RICHEDIT50W）——
            // 支持段落间距（skatch 段落分隔）与 EM_CHARFROMPOS（hover 文段联动）
            let rich_edit_lib = to_wide("msftedit.dll");
            LoadLibraryW(rich_edit_lib.as_ptr());
            let rich_class = to_wide(RICHEDIT_CLASS);
            let edit_editor = CreateWindowExW(
                0,
                rich_class.as_ptr(),
                w!(""),
                WS_CHILD | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_WANTRETURN as u32
                    | WS_VSCROLL,
                0,
                0,
                0,
                0,
                input_hwnd,
                null_mut(),
                hinstance,
                null_mut(),
            );
            let old2 = SetWindowLongPtrW(
                edit_editor,
                GWLP_WNDPROC,
                edit_wndproc as *const () as isize,
            );
            sh.old_edit_editor_proc = Some(std::mem::transmute::<isize, _>(old2));
            sh.edit_editor = edit_editor;

            // 编辑器头部文件名输入框（单行，可改名；Enter 保存退出）
            let rename_edit = CreateWindowExW(
                0,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                input_hwnd,
                null_mut(),
                hinstance,
                null_mut(),
            );
            let old3 = SetWindowLongPtrW(
                rename_edit,
                GWLP_WNDPROC,
                edit_wndproc as *const () as isize,
            );
            sh.old_rename_edit_proc = Some(std::mem::transmute::<isize, _>(old3));
            sh.rename_edit = rename_edit;

            // 深色主题画刷（输入框/编辑器背景）
            sh.dark_brush = CreateSolidBrush(rgb(40, 43, 50));

            (edit, sh.input_font)
        });
        SendMessageW(edit, WM_SETFONT, input_font as WPARAM, 1);
        let edit_editor = SHELL.with(|s| s.borrow().edit_editor);
        // RichEdit 背景直接设置（深色主题；不依赖父窗口 WM_CTLCOLOR）
        // 铁律：SendMessageW 必须在 SHELL 借用之外
        SendMessageW(edit_editor, EM_SETBKGNDCOLOR, 0, rgb(40, 43, 50) as isize);
        SendMessageW(edit_editor, WM_SETFONT, input_font as WPARAM, 1);
        let rename_edit = SHELL.with(|s| s.borrow().rename_edit);
        SendMessageW(rename_edit, WM_SETFONT, input_font as WPARAM, 1);

        // 设置对话框（初始隐藏）
        let settings_class = to_wide(SETTINGS_CLASS);
        let settings_hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            settings_class.as_ptr(),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if settings_hwnd.is_null() {
            fatal("创建设置窗口失败");
        }
        let (settings_edit, settings_token_edit, input_font2) = SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            sh.settings_hwnd = settings_hwnd;
            let edit = CreateWindowExW(
                0,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                settings_hwnd,
                null_mut(),
                hinstance,
                null_mut(),
            );
            let old = SetWindowLongPtrW(
                edit,
                GWLP_WNDPROC,
                settings_edit_wndproc as *const () as isize,
            );
            sh.old_settings_edit_proc = Some(std::mem::transmute::<isize, _>(old));
            sh.settings_edit = edit;

            // 令牌输入框（与服务地址共用子类化过程：Enter 确定 / Esc 取消）
            let token_edit = CreateWindowExW(
                0,
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
                0,
                0,
                0,
                0,
                settings_hwnd,
                null_mut(),
                hinstance,
                null_mut(),
            );
            let old2 = SetWindowLongPtrW(
                token_edit,
                GWLP_WNDPROC,
                settings_edit_wndproc as *const () as isize,
            );
            sh.old_settings_token_edit_proc = Some(std::mem::transmute::<isize, _>(old2));
            sh.settings_token_edit = token_edit;
            (edit, token_edit, sh.input_font)
        });
        SendMessageW(settings_edit, WM_SETFONT, input_font2 as WPARAM, 1);
        SendMessageW(settings_token_edit, WM_SETFONT, input_font2 as WPARAM, 1);

        // 轻提示（toast）窗口：右上角自动隐藏；快捷键设置对话框（初始隐藏）
        let toast_class = to_wide(TOAST_CLASS);
        let toast_hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            toast_class.as_ptr(),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        let hotkey_class = to_wide(HOTKEY_CLASS);
        let hotkey_hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            hotkey_class.as_ptr(),
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        );
        if toast_hwnd.is_null() || hotkey_hwnd.is_null() {
            fatal("创建提示窗口失败");
        }
        SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            sh.toast_hwnd = toast_hwnd;
            sh.hotkey_hwnd = hotkey_hwnd;
        });

        // 注册全局快捷键（配置的或默认）；失败只提示，托盘菜单仍可激活
        let hk = SHELL.with(|s| s.borrow().hotkey);
        if let Some(hk) = hk {
            if !register_hotkey(tray_hwnd, &hk) {
                let wide = to_wide(&format!(
                    "快捷键 {} 注册失败（可能被其他程序占用），可在托盘菜单重新设置。",
                    hotkey::format(&hk)
                ));
                MessageBoxW(null_mut(), wide.as_ptr(), w!("anm-tray-win"), MB_ICONERROR);
            }
        }

        // 消息循环
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        std::process::exit(0);
    }
}

/// 注册一个窗口类（类名与 WndProc 由参数指定；类名会被系统拷贝）。
fn register_class(
    hinstance: HINSTANCE,
    class_name: PCWSTR,
    wndproc: Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>,
) {
    unsafe {
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.style = CS_HREDRAW | CS_VREDRAW;
        wc.lpfnWndProc = wndproc;
        wc.hInstance = hinstance;
        wc.hIcon = LoadIconW(hinstance, APP_ICON_ID as PCWSTR);
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.lpszClassName = class_name;
        if RegisterClassW(&wc) == 0 {
            fatal("注册窗口类失败");
        }
    }
}

/// 创建一种字体（负高度 = 像素大小；中文回退由 GDI 的字体链接处理）。
fn create_font(px: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            w!("Microsoft YaHei UI"),
        )
    }
}

/// 致命错误：弹窗提示后退出（托盘程序没有控制台）。
fn fatal(msg: &str) -> ! {
    unsafe {
        let wide = to_wide(msg);
        MessageBoxW(null_mut(), wide.as_ptr(), w!("anm-tray-win"), MB_ICONERROR);
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// 隐藏主窗口：热键 + 托盘
// ---------------------------------------------------------------------------

/// 隐藏主窗口过程：处理热键（WM_HOTKEY）、托盘回调（TRAY_MSG）与销毁清理。
unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            // 全局热键注册移出（启动流程统一注册，见 run()）；此处只挂托盘图标
            add_tray_icon(hwnd);
            0
        }
        WM_HOTKEY if wparam as i32 == HOTKEY_ID => {
            // 切换语义：可见 → 隐藏（取消激活）；不可见 → 显示
            let visible = SHELL.with(|s| s.borrow().overlay_visible);
            if visible {
                deactivate_overlay();
            } else {
                activate_overlay();
            }
            0
        }
        TRAY_MSG => {
            match lparam as u32 {
                WM_LBUTTONDOWN => activate_overlay(),
                WM_RBUTTONUP => show_tray_menu(hwnd),
                _ => {}
            }
            0
        }
        MSG_OVERVIEW_DONE => {
            apply_overview_result();
            0
        }
        WM_DESTROY => {
            unsafe {
                UnregisterHotKey(hwnd, HOTKEY_ID);
                remove_tray_icon(hwnd);
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 向系统托盘添加图标。
fn add_tray_icon(hwnd: HWND) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = TRAY_MSG;
        nid.hIcon = LoadIconW(GetModuleHandleW(null_mut()), APP_ICON_ID as PCWSTR);
        let tip = to_wide("anm-tray-win（左键激活 · 右键菜单）");
        copy_wide_into(&tip, &mut nid.szTip);
        Shell_NotifyIconW(NIM_ADD, &mut nid);
    }
}

/// 从系统托盘移除图标（退出时清理）。
fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        Shell_NotifyIconW(NIM_DELETE, &mut nid);
    }
}

/// 托盘右键菜单：显示 / 设置服务地址 / 设置快捷键 / 退出
/// （TPM_RETURNCMD 直接返回被选中的命令 id）。
fn show_tray_menu(hwnd: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, MENU_ACTIVATE, w!("显示"));
        AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("设置服务地址…"));
        AppendMenuW(menu, MF_STRING, MENU_HOTKEY, w!("设置快捷键…"));
        AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            hwnd,
            null_mut(),
        );
        DestroyMenu(menu);
        match cmd as usize {
            MENU_ACTIVATE => activate_overlay(),
            MENU_SETTINGS => open_settings_dialog(),
            MENU_HOTKEY => open_hotkey_dialog(),
            MENU_EXIT => {
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// 覆盖层：激活 / 取消激活 / 渲染
// ---------------------------------------------------------------------------

/// 激活覆盖层：**立即**显示窗口（默认居中输入框，无卡片），随后**后台线程**
/// 拉取 Overview——服务不可达时界面照样秒开，稍后展示错误而非冻结 UI。
fn activate_overlay() {
    let (hwnd, input_hwnd, edit, sw, sh) = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        // 输入框矩形始终有效：先按屏幕尺寸算默认居中位置
        let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        sh.core.input_rect = Rect {
            x: (sw - sh.params.input_w) / 2,
            y: (screen_h - sh.params.input_h) / 2,
            w: sh.params.input_w,
            h: sh.params.input_h,
        };
        sh.core.cards.clear();
        sh.core.hover = None;
        sh.core.drag = None;
        sh.core.result.clear();
        sh.core.error.clear();
        sh.core.editor = None;
        sh.overlay_visible = true;
        (sh.hwnd, sh.input_hwnd, sh.edit, sw, screen_h)
    });

    unsafe {
        // 借用已释放：变暗层全屏置顶，输入窗居中（单行模式）
        let edit_editor = SHELL.with(|s| s.borrow().edit_editor);
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, sw, sh, SWP_SHOWWINDOW);
        let input_rect = SHELL.with(|s| s.borrow().core.input_rect);
        SetWindowPos(
            input_hwnd,
            HWND_TOPMOST,
            input_rect.x,
            input_rect.y,
            input_rect.w,
            input_rect.h,
            SWP_SHOWWINDOW,
        );
        set_window_rounded(input_hwnd, input_rect.w, input_rect.h, 10);
        SetWindowPos(edit, null_mut(), 0, 0, input_rect.w, input_rect.h, 0);
        SetWindowTextW(edit, w!(""));
        ShowWindow(edit, SW_SHOW);
        ShowWindow(edit_editor, SW_HIDE);
        ShowWindow(hwnd, SW_SHOW);
        ShowWindow(input_hwnd, SW_SHOW);
        SetForegroundWindow(input_hwnd);
        SetFocus(edit);
    }
    render_overlay(hwnd);
    fetch_overview_async();
}

/// 后台线程拉取 Overview：结果放入共享槽，经 `MSG_OVERVIEW_DONE` 通知主线程。
/// 已在途时不重复发起。
fn fetch_overview_async() {
    if OVERVIEW_FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    // 刷新前先保存当前卡片滚动位置（否则重建后滚动条归零）
    let scrolls_now: Vec<(String, usize)> = SHELL.with(|s| {
        s.borrow()
            .core
            .cards
            .iter()
            .filter(|c| !c.temp && c.scroll > 0)
            .map(|c| (c.dir_path.clone(), c.scroll))
            .collect()
    });
    if !scrolls_now.is_empty() {
        SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            for (dir, sc) in scrolls_now {
                sh.core.scrolls.insert(dir, sc);
            }
        });
    }
    // HWND 非 Send：转 usize 进线程，用回再转
    let tray = SHELL.with(|s| s.borrow().tray_hwnd as usize);
    std::thread::spawn(move || {
        // 总览与 skatch 各一次 IPC（skatch 失败不阻塞主界面）
        let dirs = ipc::call(&Request::Overview).and_then(|data| {
            serde_json::from_value::<Vec<DirOverview>>(data)
                .map_err(|e| anyhow::anyhow!("总览数据解析失败: {e}"))
        });
        let skatch = ipc::call(&Request::Skatch).ok().and_then(|data| {
            let v = data.as_object()?;
            let path = v.get("path")?.as_str()?.to_string();
            let root = v.get("root").and_then(|r| r.as_str()).unwrap_or("").to_string();
            let segments = v
                .get("segments")?
                .as_array()?
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect::<Vec<_>>();
            Some((path, root, segments))
        });
        let result = match dirs {
            Ok(dirs) => Ok(HomeData { dirs, skatch }),
            Err(e) => Err(format!("{e:#}")),
        };
        *HOME_RESULT.lock().unwrap() = Some(result);
        unsafe {
            PostMessageW(tray as HWND, MSG_OVERVIEW_DONE, 0, 0);
        }
    });
}

/// 处理后台拉取结果：覆盖层仍可见才应用卡片与布局并重绘；
/// 已取消激活则丢弃（下次激活会重新拉取）。
fn apply_overview_result() {
    OVERVIEW_FETCHING.store(false, Ordering::SeqCst);
    let result = HOME_RESULT.lock().unwrap().take();
    let Some(result) = result else {
        return;
    };
    match result {
        Ok(home) => {
            let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            let hwnd = SHELL.with(|s| {
                let mut st = s.borrow_mut();
                if !st.overlay_visible {
                    return None;
                }
                let params = st.params.clone();
                let (input_rect, mut cards) = cards::layout(sw, sh, &home.dirs, &params);
                // skatch 卡片追加在末尾（最上层；暖色强调、宽度加宽，见渲染）
                if let Some((path, _root, segments)) = &home.skatch {
                    if !segments.is_empty() {
                        cards.push(cards::build_skatch_card(path, segments, sw, sh, &params));
                    }
                }
                st.core.cards = cards;
                st.core.input_rect = input_rect;
                st.core.result.clear();
                st.core.error.clear();
                // 应用记忆位置与滚动（临时卡片不参与）
                st.core.apply_positions(sw, sh);
                st.core.apply_scrolls(&params);
                Some(st.hwnd)
            });
            if let Some(hwnd) = hwnd {
                render_overlay(hwnd);
            }
        }
        Err(e) => set_error(e),
    }
}

/// 取消激活：退出编辑器（自动保存/改名）→ 清空临时状态 → 隐藏窗口。
fn deactivate_overlay() {
    // 文件名输入框文本先读（可变借用内读 EDIT 会重入窗口过程 → 铁律）
    let (hwnd, input_hwnd, edit, new_name) = SHELL.with(|s| {
        let st = s.borrow();
        (
            st.hwnd,
            st.input_hwnd,
            st.edit,
            if st.core.editor.is_some() {
                read_edit_text(st.rename_edit)
            } else {
                String::new()
            },
        )
    });
    SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        let editor_msg = if sh.core.editor.is_some() {
            model::exit_editor(&mut sh.core, &new_name)
        } else {
            String::new()
        };
        if !editor_msg.is_empty() {
            sh.core.result = editor_msg;
        }
        sh.core.clear_transient();
        sh.core.subcard_top = None;
        sh.overlay_visible = false;
        sh.pending_new_note = None;
        sh.row_press = None;
        sh.nav_hover = None;
    });
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        ShowWindow(input_hwnd, SW_HIDE);
        SetWindowTextW(edit, w!(""));
    }
}

/// 把输入框窗口显式提到变暗层之上（不移动、不改变大小、不抢激活）。
fn raise_input_window() {
    let input_hwnd = SHELL.with(|s| s.borrow().input_hwnd);
    unsafe {
        SetWindowPos(
            input_hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

// ---------------------------------------------------------------------------
// 输入：提交（anw / 斜杠命令）
// ---------------------------------------------------------------------------

/// 提交输入框内容（launcher 模式回车）：交给核心处理（anw 或斜杠命令），
/// 执行返回的 Action，结果显示在输入框下方。
fn submit_input() {
    let edit = SHELL.with(|s| s.borrow().edit);
    let text = read_edit_text(edit);

    let outcome = commands::run_input(&text);
    let (hwnd, clear_input) = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.core.result = outcome.result;
        sh.core.error.clear();
        (sh.hwnd, sh.core.result.starts_with("已写入"))
    });
    if clear_input {
        unsafe { SetWindowTextW(edit, w!("")) };
    }
    if let Some(action) = outcome.action {
        execute_action(action);
    }
    render_overlay(hwnd);
}

/// 读取 EDIT 控件全文（UTF-16 → String）。
fn read_edit_text(edit: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(edit);
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

// ---------------------------------------------------------------------------
// 动作执行
// ---------------------------------------------------------------------------

/// 执行核心返回的动作（在 SHELL 借用之外调用）。
fn execute_action(action: Action) {
    match action {
        Action::None => {}
        Action::Deactivate => deactivate_overlay(),
        Action::Open(path) => {
            if open_with_default_handler(&path) {
                deactivate_overlay();
            } else {
                // 系统打不开（典型：文件/目录只在服务端机器上）→ 提示，停留在覆盖层
                show_toast(format!(
                    "无法打开：{path}（此路径只在服务端机器上，本机没有对应文件）"
                ));
            }
        }
        Action::EnterEditor(core_path) => {
            // 再次点击正在编辑的文件名 = 关闭编辑器（与 ✕ 一致）
            let same = SHELL.with(|s| {
                s.borrow()
                    .core
                    .editor
                    .as_ref()
                    .map_or(false, |e| e.path == core_path)
            });
            if same {
                exit_editor_mode();
            } else {
                enter_editor_mode(&core_path, None);
            }
        }
        Action::OpenTempCard { dir_path, at } => open_temp_card(&dir_path, at),
        Action::NewNote(dir) => start_new_note(&dir),
        Action::MoveNote { from, to_dir } => {
            match ipc::call(&Request::MoveNote {
                from: from.clone(),
                to_dir: to_dir.clone(),
            }) {
                Ok(_) => {
                    SHELL.with(|s| {
                        let mut sh = s.borrow_mut();
                        sh.core.result = format!("已移动 → {to_dir}");
                        sh.core.error.clear();
                    });
                    let hwnd = SHELL.with(|s| s.borrow().hwnd);
                    render_overlay(hwnd);
                    fetch_overview_async();
                }
                Err(e) => set_error(format!("{e:#}")),
            }
        }
        Action::SkatchExtract { dir, index } => {
            match ipc::call(&Request::SkatchExtract {
                dir: dir.clone(),
                index,
            }) {
                Ok(data) => {
                    let p = data
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&dir)
                        .to_string();
                    SHELL.with(|s| {
                        let mut sh = s.borrow_mut();
                        sh.core.result = format!("已抽取为文件 {p}");
                        sh.core.error.clear();
                    });
                    let hwnd = SHELL.with(|s| s.borrow().hwnd);
                    render_overlay(hwnd);
                    fetch_overview_async();
                }
                Err(e) => set_error(format!("{e:#}")),
            }
        }
        Action::SkatchInsert { from } => {
            match ipc::call(&Request::SkatchInsert { from: from.clone() }) {
                Ok(_) => {
                    SHELL.with(|s| {
                        let mut sh = s.borrow_mut();
                        sh.core.result = format!("已并入 skatch ← {from}");
                        sh.core.error.clear();
                    });
                    let hwnd = SHELL.with(|s| s.borrow().hwnd);
                    render_overlay(hwnd);
                    fetch_overview_async();
                }
                Err(e) => set_error(format!("{e:#}")),
            }
        }
        Action::SavePositions => save_positions(),
    }
}

// ---------------------------------------------------------------------------
// 新建笔记（标题行「+」按钮）
// ---------------------------------------------------------------------------

/// 进入新建笔记模式：输入框预填目录前缀，回车创建 / Esc 取消。
fn start_new_note(dir: &str) {
    let (hwnd, edit) = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.pending_new_note = Some(dir.to_string());
        sh.core.result = format!("输入文件名（回车创建，Esc 取消）→ {dir}/");
        sh.core.error.clear();
        (sh.hwnd, sh.edit)
    });
    unsafe {
        let wide = to_wide(&format!("{dir}/"));
        SetWindowTextW(edit, wide.as_ptr());
        // 光标移到末尾：EM_SETSEL 按 UTF-16 单元计数（字节数在中文路径下会错位）
        let sel = format!("{dir}/").encode_utf16().count();
        SendMessageW(edit, EM_SETSEL, sel as usize, sel as isize);
        SetFocus(edit);
    }
    render_overlay(hwnd);
}

/// 提交新建：解析文件名（支持预填前缀）→ IPC CreateNote → 刷新总览。
fn submit_new_note() {
    let dir = SHELL.with(|s| s.borrow_mut().pending_new_note.take());
    let Some(dir) = dir else {
        return;
    };
    let edit = SHELL.with(|s| s.borrow().edit);
    let text = read_edit_text(edit);
    let prefix = format!("{dir}/");
    // 用户改了前缀也没关系：整个输入当文件名（core 侧 sanitize 兜底）
    let name = text.strip_prefix(&prefix).unwrap_or(text.trim()).trim().to_string();
    let mut created = false;
    if name.is_empty() {
        SHELL.with(|s| s.borrow_mut().core.error = "请输入文件名".to_string());
    } else {
        match ipc::call(&Request::CreateNote {
            dir: dir.clone(),
            title: name.clone(),
        }) {
            Ok(data) => {
                let p = data
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&dir)
                    .to_string();
                SHELL.with(|s| {
                    let mut sh = s.borrow_mut();
                    sh.core.result = format!("已创建 {p}");
                    sh.core.error.clear();
                });
                unsafe { SetWindowTextW(edit, w!("")) };
                created = true;
            }
            Err(e) => SHELL.with(|s| s.borrow_mut().core.error = format!("{e:#}")),
        }
    }
    let hwnd = SHELL.with(|s| s.borrow().hwnd);
    render_overlay(hwnd);
    if created {
        fetch_overview_async();
    }
}

/// 取消新建笔记模式（Esc）：清状态、清输入框、恢复普通 launcher。
fn cancel_new_note() {
    let (hwnd, edit) = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.pending_new_note = None;
        sh.core.result.clear();
        sh.core.error.clear();
        (sh.hwnd, sh.edit)
    });
    unsafe { SetWindowTextW(edit, w!("")) };
    render_overlay(hwnd);
}

/// 打开临时子卡片：同一目录已存在临时卡 → **关闭**（与内置编辑器一致）；
/// 否则 IPC 拉取子目录总览 → 构建临时卡片 → 加入状态 → 渲染。
fn open_temp_card(dir_path: &str, at: (i32, i32)) {
    // 先查是否已有同目录临时卡（不拉数据）：有 → 关闭（并清除其置顶）
    let exists = SHELL.with(|s| {
        s.borrow()
            .core
            .cards
            .iter()
            .any(|c| c.temp && c.dir_path == dir_path)
    });
    if exists {
        let hwnd = SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            sh.core.cards.retain(|c| !(c.temp && c.dir_path == dir_path));
            if sh.core.subcard_top.as_deref() == Some(dir_path) {
                sh.core.subcard_top = None;
            }
            sh.hwnd
        });
        render_overlay(hwnd);
        return;
    }
    // 没有 → 拉数据开新卡
    let result = ipc::call(&Request::OverviewDir {
        dir: dir_path.to_string(),
    });
    match result {
        Ok(data) => match serde_json::from_value::<DirOverview>(data) {
            Ok(ov) => {
                let (sw, screen_h) = unsafe {
                    (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
                };
                let hwnd = SHELL.with(|s| {
                    let mut sh = s.borrow_mut();
                    let card = cards::build_temp_card(&ov, at, sw, screen_h, &sh.params);
                    // 子卡打开即置顶（保持到父/子卡都不再 hover）
                    sh.core.subcard_top = Some(card.dir_path.clone());
                    sh.core.cards.push(card);
                    sh.hwnd
                });
                render_overlay(hwnd);
            }
            Err(e) => set_error(format!("子目录数据解析失败: {e}")),
        },
        Err(e) => set_error(format!("{e:#}")),
    }
}

/// 设置错误提示并重绘。
fn set_error(msg: String) {
    let hwnd = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.core.error = msg;
        sh.hwnd
    });
    render_overlay(hwnd);
}

// ---------------------------------------------------------------------------
// 内置临时编辑器
// ---------------------------------------------------------------------------

/// 进入编辑器模式：核心经 IPC 读取笔记内容 → 输入框窗口放大为多行编辑框，
/// 显示导航条（skatch 模式）/按钮。内容读取/保存全部走 core（跨机器可编辑）。
///
/// `skatch_index`：skatch 段落点开时传入段落下标（定位到该段并滚动导航条）。
fn enter_editor_mode(core_path: &str, skatch_index: Option<usize>) {
    // 核心经 IPC 读取（失败 → 错误提示，不进入编辑模式）
    let read_ok = SHELL.with(|s| {
        model::enter_editor(&mut s.borrow_mut().core, core_path, skatch_index)
    });
    if let Err(e) = read_ok {
        set_error(e);
        return;
    }
    let (hwnd, input_hwnd, edit, edit_editor, rename_edit, content, file_name) = SHELL.with(|s| {
        let st = s.borrow();
        let ed = st.core.editor.as_ref();
        (
            st.hwnd,
            st.input_hwnd,
            st.edit,
            st.edit_editor,
            st.rename_edit,
            ed.map(|e| e.content.clone()).unwrap_or_default(),
            ed.and_then(|e| {
                std::path::Path::new(&e.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
            })
            .unwrap_or_default(),
        )
    });
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    unsafe {
        // 借用已释放：放大窗口、切到多行编辑框、载入文本、显示按钮
        set_window_rounded(input_hwnd, EDITOR_W, EDITOR_H, 12);
        SetWindowPos(
            input_hwnd,
            HWND_TOPMOST,
            (sw - EDITOR_W) / 2,
            (sh - EDITOR_H) / 2,
            EDITOR_W,
            EDITOR_H,
            SWP_SHOWWINDOW,
        );
        // 编辑区：头部信息栏之下、按钮行之上
        let edit_rect = editor_edit_rect(EDITOR_W, EDITOR_H);
        SetWindowPos(
            edit_editor,
            null_mut(),
            edit_rect.x,
            edit_rect.y,
            edit_rect.w,
            edit_rect.h,
            0,
        );
        // 头部第一行：文件名输入框（可改名），右端让出保存/✕
        SetWindowPos(
            rename_edit,
            null_mut(),
            14,
            8,
            EDITOR_W - 148,
            26,
            SWP_SHOWWINDOW,
        );
        let name_wide = to_wide(&file_name);
        SetWindowTextW(rename_edit, name_wide.as_ptr());
        let wide = to_wide(&content);
        SetWindowTextW(edit_editor, wide.as_ptr());
        // 内容加载后再应用白色文字（SCF_ALL 对空文本无效——须在 WM_SETTEXT 之后）
        set_rich_text_white(edit_editor);
        ShowWindow(edit, SW_HIDE);
        ShowWindow(rename_edit, SW_SHOW);
        ShowWindow(edit_editor, SW_SHOW);
        SHELL.with(|s| s.borrow_mut().btn_hover = 0);
        SetFocus(edit_editor);
    }
    // skatch 段落分隔 + 定位（借用外）：先逐段设置段后间距并清除选择，
    // 再滚动定位到所选段落（顺序不可反——间距设置会移动光标）
    if let Some(idx) = skatch_index {
        let content = SHELL.with(|s| {
            s.borrow()
                .core
                .editor
                .as_ref()
                .map(|ed| ed.content.clone())
                .unwrap_or_default()
        });
        unsafe {
            // RichEdit 内部为 CRLF：先把内容转成显示文本（\n → \r\n），
            // 后续所有偏移（EM_SETSEL / EM_CHARFROMPOS）都基于它——
            // 这是 Windows 平台层的换算，共享层保持 LF 语义（wayland 版不需要）。
            let display = content.replace('\n', "\r\n");
            // 1) 段落分隔：按行分段，逐行把**光标**放到行首（不产生选择、
            //    不滚动视图），EM_SETPARAFORMAT wParam=0 应用于光标所在行。
            let mut off16 = 0usize;
            for line in display.split('\n') {
                let len16 = line.encode_utf16().count();
                SendMessageW(edit_editor, EM_SETSEL, off16 as usize, off16 as isize);
                let mut pf: Parafmt2 = std::mem::zeroed();
                pf.cb_size = size_of::<Parafmt2>() as u32;
                pf.dw_mask = PFM_SPACEBEFORE | PFM_SPACEAFTER;
                pf.dy_space_before = 40;
                pf.dy_space_after = 120; // twips：行后约 6pt，形成明显行间分隔
                SendMessageW(
                    edit_editor,
                    EM_SETPARAFORMAT,
                    0, // 应用于光标所在段落
                    &pf as *const Parafmt2 as isize,
                );
                off16 += len16 + 1; // 行(含行尾 \r) + \n
            }
            // 2) 定位到所选行（光标置于行首，滚动到可见；偏移同为 UTF-16）
            if let Some(off) = model::skatch_segment_offset(&display, idx) {
                SendMessageW(edit_editor, EM_SETSEL, off as usize, off as isize);
                SendMessageW(edit_editor, EM_SCROLLCARET, 0, 0);
            }
        }
    }
    raise_input_window();
    render_overlay(hwnd);
}

/// 退出编辑器模式：读回文本与文件名 → 同步核心 → 退出（有改动/改名自动保存）→ 恢复单行。
fn exit_editor_mode() {
    let (hwnd, input_hwnd, edit, edit_editor, rename_edit, text, new_name) = SHELL.with(|s| {
        let st = s.borrow();
        (
            st.hwnd,
            st.input_hwnd,
            st.edit,
            st.edit_editor,
            st.rename_edit,
            read_edit_text(st.edit_editor),
            read_edit_text(st.rename_edit),
        )
    });
    let msg = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        model::editor_set_content(&mut sh.core, &text);
        model::exit_editor(&mut sh.core, &new_name)
    });
    let input_rect = SHELL.with(|s| s.borrow().core.input_rect);
    unsafe {
        // 恢复单行输入模式
        set_window_rounded(input_hwnd, input_rect.w, input_rect.h, 10);
        SetWindowPos(
            input_hwnd,
            HWND_TOPMOST,
            input_rect.x,
            input_rect.y,
            input_rect.w,
            input_rect.h,
            SWP_SHOWWINDOW,
        );
        SetWindowPos(edit, null_mut(), 0, 0, input_rect.w, input_rect.h, 0);
        SetWindowTextW(edit, w!(""));
        ShowWindow(edit_editor, SW_HIDE);
        ShowWindow(rename_edit, SW_HIDE);
        ShowWindow(edit, SW_SHOW);
        SHELL.with(|s| s.borrow_mut().btn_hover = 0);
        SetFocus(edit);
    }
    if !msg.is_empty() {
        SHELL.with(|s| s.borrow_mut().core.result = msg);
        // 有保存/改名才刷新总览（无改动的预览关闭不刷新，避免卡片重建）
        fetch_overview_async();
    }
    render_overlay(hwnd);
}

/// 保存编辑器改动（Ctrl+S；含文件名改动）。
fn save_editor_ui() {
    let (hwnd, text, new_name) = SHELL.with(|s| {
        let st = s.borrow();
        (
            st.hwnd,
            read_edit_text(st.edit_editor),
            read_edit_text(st.rename_edit),
        )
    });
    let input_hwnd = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        model::editor_set_content(&mut sh.core, &text);
        let m = model::save_editor(&mut sh.core, &new_name);
        sh.core.result = m;
        sh.input_hwnd
    });
    // 改名会改变编辑器头部路径显示 → 重绘输入框窗口
    unsafe { InvalidateRect(input_hwnd, null_mut(), 0) };
    render_overlay(hwnd);
    // 改名/保存后总览可能变化（文件名行）→ 后台刷新卡片
    fetch_overview_async();
}

/// 「打开所在位置」：在资源管理器中打开正在编辑文件所在的目录。
/// 目录在服务端机器上时本机打不开（远程模式），给出提示。
fn open_editor_location() {
    let local = SHELL.with(|s| s.borrow().core.editor.as_ref().map(|ed| ed.path.clone()));
    if let Some(local) = local {
        if let Some(parent) = std::path::Path::new(&local).parent() {
            if !open_with_default_handler(&parent.to_string_lossy()) {
                show_toast("无法打开所在位置：笔记在服务端机器上，本机没有对应文件夹".to_string());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 覆盖层鼠标/滚轮处理（调核心）
// ---------------------------------------------------------------------------

/// 覆盖层窗口过程。
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            render_overlay(hwnd);
            0
        }
        // 点击变暗层时拒绝激活：否则变暗层会被提升到输入框窗口之上
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let (action, repaint) = on_lbuttondown(hwnd, x, y);
            if repaint {
                render_overlay(hwnd);
            }
            execute_action(action);
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            if on_mouse_move(hwnd, x, y) {
                render_overlay(hwnd);
            }
            0
        }
        WM_LBUTTONUP => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let (action, repaint) = on_lbuttonup(x, y);
            if repaint {
                render_overlay(hwnd);
            }
            execute_action(action);
            0
        }
        WM_MOUSEWHEEL => {
            // 滚轮滚动光标下的卡片
            let delta = ((wparam >> 16) as u16 as i16) as i32;
            let mut pt: POINT = unsafe { std::mem::zeroed() };
            unsafe { GetCursorPos(&mut pt) };
            let changed = SHELL.with(|s| {
                let mut sh = s.borrow_mut();
                if let Some(hit) = cards::hit_test(pt.x, pt.y, &sh.core.cards) {
                    // 先取 params（拷贝）与卡片下标，避免与 cards 可变借用冲突
                    let params = sh.params.clone();
                    if let Some(card) = sh.core.cards.get_mut(hit.card) {
                        return cards::scroll_card(card, -(delta / 120 * 3) as isize, &params);
                    }
                }
                false
            });
            if changed {
                render_overlay(hwnd);
            }
            0
        }
        WM_MOUSELEAVE => {
            let changed = SHELL.with(|s| {
                let mut sh = s.borrow_mut();
                if sh.core.drag.is_none() && sh.core.hover.take().is_some() {
                    true
                } else {
                    false
                }
            });
            if changed {
                render_overlay(hwnd);
            }
            0
        }
        WM_CAPTURECHANGED => {
            let changed = SHELL.with(|s| {
                let mut sh = s.borrow_mut();
                let had = sh.core.drag.take().is_some() || sh.core.hover.take().is_some();
                had
            });
            if changed {
                render_overlay(hwnd);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 鼠标按下：
/// - 命中**标题栏** → 开始卡片拖动（仅标题栏可拖动整卡）；
/// - 命中**文本文件行** → **立即用内置编辑器打开**（预览即打开）；按住超过
///   单击阈值后松开 = 预览结束（关闭）；快速松开 = 正常打开；按住移动 = 拖动；
/// - 命中**非文本文件行 / 子目录行** → 记录按下（抬起时单击处理）；
/// - 未命中 → 取消激活。
fn on_lbuttondown(hwnd: HWND, x: i32, y: i32) -> (Action, bool) {
    // 记录按下状态（借用内先取数据，借用外统一赋值，避免借用冲突）；
    // enter_editor_mode / open_temp_card 在借用外调用
    let mut open_editor: Option<(String, Option<usize>)> = None;
    let mut open_subcard: Option<String> = None;
    let mut press_info: Option<(usize, usize, Option<String>, Option<usize>, bool, bool, Option<String>)> = None;
    let hit = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        let hit = cards::hit_test(x, y, &sh.core.cards);
        if let Some(hit) = hit {
            sh.core.hover = None;
            sh.row_press = None;
            if let Some(card) = sh.core.cards.get(hit.card) {
                let real = card.row_of(hit.row);
                match &card.rows[real] {
                    CardRow::DirHeader => {
                        // 标题栏：卡片拖动
                        sh.core.drag = Some(DragState {
                            card: hit.card,
                            row: hit.row,
                            grab_dx: x - card.rect.x,
                            grab_dy: y - card.rect.y,
                            start_x: x,
                            start_y: y,
                            moved: false,
                        });
                    }
                    CardRow::File { path, .. } => {
                        let skatch_index = if card.skatch { Some(real - 1) } else { None };
                        let is_text = is_text_file(path);
                        let mut was_open = false;
                        if skatch_index.is_some() {
                            // skatch 段落：总是打开/重新定位到该段
                            open_editor = Some((path.clone(), skatch_index));
                        } else if is_text {
                            // 普通文本：已打开同文件 → 不重开（松开单击时 toggle 关闭）
                            let same = sh
                                .core
                                .editor
                                .as_ref()
                                .map_or(false, |e| e.path == *path);
                            if same {
                                was_open = true;
                            } else {
                                open_editor = Some((path.clone(), None));
                            }
                        }
                        press_info = Some((
                            hit.card,
                            hit.row,
                            Some(path.clone()),
                            skatch_index,
                            is_text,
                            was_open,
                            None,
                        ));
                    }
                    CardRow::SubDir { name } => {
                        // 子目录行：按下即显示子卡（预览），松开判定保留/关闭
                        let sub_dir = format!("{}/{}", card.dir_path, name);
                        open_subcard = Some(sub_dir.clone());
                        press_info = Some((
                            hit.card,
                            hit.row,
                            None,
                            None,
                            false,
                            false,
                            Some(sub_dir),
                        ));
                    }
                }
            }
        }
        hit
    });
    if let Some((card, row, path, skatch_index, _is_text, was_open, subcard_dir)) = press_info {
        // 子卡预览：按下立即打开/关闭（toggle），并记录本次是否处于"打开"态
        let mut subcard_opened = false;
        if let Some(sub) = &open_subcard {
            open_temp_card(sub, (x + 24, y + 24));
            subcard_opened = SHELL.with(|s| {
                s.borrow()
                    .core
                    .cards
                    .iter()
                    .any(|c| c.temp && c.dir_path == *sub)
            });
        }
        SHELL.with(|s| {
            s.borrow_mut().row_press = Some(RowPress {
                card,
                row,
                start_x: x,
                start_y: y,
                moved: false,
                press_at: std::time::Instant::now(),
                path,
                skatch_index,
                was_open,
                subcard_dir,
                subcard_opened,
            });
        });
    }
    match hit {
        Some(_) => {
            unsafe {
                SetCapture(hwnd);
            }
            raise_input_window();
            // 文本文件：按下即打开编辑器（预览；skatch 带段落下标定位）
            if let Some((path, idx)) = open_editor {
                enter_editor_mode(&path, idx);
            }
            (Action::None, true)
        }
        None => (Action::Deactivate, false),
    }
}

/// 鼠标抬起：
/// - 内容行按下（文件/子目录）：预览中 → 关预览；已拖动 → 文件移动/取消；
///   未移动 → 单击（打开 / 编辑 / 开子卡）；
/// - 标题栏拖动：已位移 → 保存位置；否则点击（打开文件夹 / 加号新建）。
fn on_lbuttonup(x: i32, y: i32) -> (Action, bool) {
    // 内容行按下（先取出并释放借用；ReleaseCapture 会同步发 WM_CAPTURECHANGED）
    let press = SHELL.with(|s| s.borrow_mut().row_press.take());
    if let Some(p) = press {
        unsafe {
            ReleaseCapture();
        }
        // 子卡预览：拖动或长按松开 → 关闭按下时打开的子卡
        if (p.moved || p.press_at.elapsed().as_millis() >= CLICK_MS) && p.subcard_opened {
            close_subcard_preview(p.subcard_dir.as_deref().unwrap_or_default());
        }
        // 子目录行：开/关已由按下时完成（预览），松开不再走单击解析（防二次触发）
        if p.subcard_dir.is_some() {
            return (Action::None, true);
        }
        if p.moved {
            // 拖动结束：源是文件 → 移动/抽取/并入；子目录行拖动 → 取消
            if p.path.is_some() {
                return (move_note_action(&p, x, y), true);
            }
            return (Action::None, true);
        }
        // 未拖动：区分"长按预览"与"正常单击"
        let long_press = p.press_at.elapsed().as_millis() >= CLICK_MS;
        if long_press && p.path.is_some() {
            // 长按后松开 = 预览结束：关闭内置编辑器（若有改动会自动保存）
            let in_editor = SHELL.with(|s| s.borrow().core.editor.is_some());
            if in_editor {
                exit_editor_mode();
            }
            return (Action::None, true);
        }
        if p.path.is_some() && !long_press {
            // 快速单击文本文件：按下前已打开同文件 → toggle 关闭；否则保持；
            // 非文本文件：系统打开
            let is_text = SHELL.with(|s| {
                s.borrow()
                    .core
                    .cards
                    .get(p.card)
                    .and_then(|card| card.rows.get(card.row_of(p.row)))
                    .map(|r| match r {
                        CardRow::File { path, .. } => is_text_file(path),
                        _ => false,
                    })
                    .unwrap_or(false)
            });
            if !is_text {
                let path = p.path.clone().unwrap_or_default();
                return (
                    if path.is_empty() {
                        Action::None
                    } else {
                        Action::Open(path)
                    },
                    true,
                );
            }
            // 文本：按下前已打开同文件（普通文件）→ toggle 关闭；否则保持
            if p.was_open && p.skatch_index.is_none() {
                exit_editor_mode();
                return (Action::None, true);
            }
            return (Action::None, true); // 文本：编辑器已在按下时打开/定位
        }
        // 单击：解析真实行 → 动作
        let action = SHELL.with(|s| {
            let sh = s.borrow();
            sh.core.cards.get(p.card).and_then(|card| {
                let real = card.row_of(p.row);
                card.rows.get(real).map(|row| match row {
                    CardRow::SubDir { name } => Action::OpenTempCard {
                        dir_path: format!("{}/{}", card.dir_path, name),
                        // 以鼠标位置为基准偏移，新卡不遮挡鼠标所在处
                        at: (x + 24, y + 24),
                    },
                    CardRow::File { path, .. } => {
                        if is_text_file(path) {
                            // 内容由 core 经 IPC 提供，路径直接用 core 侧路径（跨机器可编辑）
                            Action::EnterEditor(path.clone())
                        } else {
                            Action::Open(path.clone())
                        }
                    }
                    CardRow::DirHeader => Action::Open(card.dir_path.clone()),
                })
            })
        });
        return (action.unwrap_or(Action::Deactivate), false);
    }

    // 标题栏卡片拖动
    let drag = SHELL.with(|s| s.borrow_mut().core.drag.take());
    let Some(drag) = drag else {
        return (Action::None, false);
    };
    unsafe {
        ReleaseCapture();
    }
    raise_input_window();

    if drag.moved {
        save_positions();
        return (Action::None, false);
    }
    // 点击标题栏：先判右上角加号（位置固定，滚动后仍生效），再打开文件夹
    let action = SHELL.with(|s| {
        let sh = s.borrow();
        sh.core.cards.get(drag.card).and_then(|card| {
            let params = sh.params.clone();
            if cards::title_plus_rect(card, &params).map_or(false, |p| p.contains(x, y)) {
                return Some(Action::NewNote(card.dir_path.clone()));
            }
            Some(Action::Open(card.dir_path.clone()))
        })
    });
    (action.unwrap_or(Action::Deactivate), false)
}

/// 关闭子卡预览（长按/拖动松开时）：移除临时卡并清除其置顶。
fn close_subcard_preview(dir_path: &str) {
    let hwnd = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.core.cards.retain(|c| !(c.temp && c.dir_path == dir_path));
        if sh.core.subcard_top.as_deref() == Some(dir_path) {
            sh.core.subcard_top = None;
        }
        sh.hwnd
    });
    render_overlay(hwnd);
}

/// 文件行拖动结束：
/// - 松在**目录卡片**上：普通文件 → 跨目录移动；skatch 段落 → 抽取成独立文件；
/// - 松在 **skatch 卡片**上：普通文件 → 内容并入 skatch 末尾（删除原文件）；
///   skatch 段落 → 取消；
/// - 松在空白处 → 取消。
fn move_note_action(p: &RowPress, x: i32, y: i32) -> Action {
    let src_path = p.path.clone().unwrap_or_default();
    let src_is_skatch = p.skatch_index.is_some();
    let (target_dir, target_is_skatch) = SHELL.with(|s| {
        let sh = s.borrow();
        let hit = cards::hit_test(x, y, &sh.core.cards);
        match hit {
            Some(h) => (
                sh.core.cards[h.card].dir_path.clone(),
                sh.core.cards[h.card].skatch,
            ),
            None => (String::new(), false),
        }
    });
    if target_dir.is_empty() {
        return Action::None; // 空白处
    }
    if target_is_skatch {
        // 目标 = skatch：普通文件 → 并入；skatch 段落 → 取消
        if src_is_skatch {
            return Action::None;
        }
        return Action::SkatchInsert { from: src_path };
    }
    // 目标 = 目录卡片
    if src_is_skatch {
        // skatch 段落 → 抽取成独立文件
        return Action::SkatchExtract {
            dir: target_dir,
            index: p.skatch_index.unwrap_or(0),
        };
    }
    let src_dir = SHELL.with(|s| {
        s.borrow()
            .core
            .cards
            .get(p.card)
            .map(|c| c.dir_path.clone())
            .unwrap_or_default()
    });
    if src_dir == target_dir {
        return Action::None; // 同目录
    }
    Action::MoveNote {
        from: src_path,
        to_dir: target_dir,
    }
}

/// 鼠标移动：标题栏拖动 → 移动卡片；文件/子目录行按下 → 超阈值进入拖动；
/// 否则更新悬停。返回是否需要重绘。
fn on_mouse_move(hwnd: HWND, x: i32, y: i32) -> bool {
    // 拖动开始瞬间需要关闭按下时打开的编辑器预览（借用外调用）
    let mut close_editor_preview = false;
    let changed = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        // 内容行按下：超过阈值 → 标记 moved（进入拖动）；按下时打开的
        // 编辑器预览随之关闭（无改动，不触发总览刷新）
        // 先取编辑器状态（避免与 row_press 可变借用冲突）
        let editor_open = sh.core.editor.is_some();
        if let Some(rp) = sh.row_press.as_mut() {
            if !rp.moved && (x - rp.start_x).abs() + (y - rp.start_y).abs() >= DRAG_THRESHOLD {
                rp.moved = true;
                close_editor_preview = editor_open;
            }
            if rp.moved {
                return true; // 拖动中：目标高亮需随鼠标重绘
            }
        }
        // 先拷贝拖动信息，避免同时可变借用 drag 与 cards
        let drag_info = sh
            .core
            .drag
            .as_ref()
            .map(|d| (d.card, d.grab_dx, d.grab_dy, d.start_x, d.start_y));
        if let Some((card_idx, grab_dx, grab_dy, start_x, start_y)) = drag_info {
            let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let shh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            let mut moved = false;
            if let Some(card) = sh.core.cards.get_mut(card_idx) {
                let dx = x - grab_dx - card.rect.x;
                let dy = y - grab_dy - card.rect.y;
                cards::translate_card(card, dx, dy, sw, shh);
                moved = (x - start_x).abs() + (y - start_y).abs() >= DRAG_THRESHOLD;
            }
            if moved {
                if let Some(drag) = sh.core.drag.as_mut() {
                    drag.moved = true;
                }
            }
            return true;
        }
        // 悬停高亮（拖动时不更新）；hover 过的卡片进入置顶层（保持置顶）
        let hover = cards::hit_test(x, y, &sh.core.cards);
        if hover != sh.core.hover {
            if let Some(h) = hover {
                let dp = sh.core.cards[h.card].dir_path.clone();
                sh.core.top.retain(|t| *t != dp);
                sh.core.top.push(dp.clone());
                // 子卡置顶：hover 父卡或子卡 → 保持；否则退出置顶
                if let Some(sub) = sh.core.subcard_top.clone() {
                    let parent = sub
                        .rsplit_once('/')
                        .map(|(p, _)| p.to_string())
                        .unwrap_or_default();
                    if dp != sub && dp != parent {
                        sh.core.subcard_top = None;
                    }
                }
            } else {
                // 移出所有卡片：子卡不再置顶
                sh.core.subcard_top = None;
            }
            sh.core.hover = hover;
            unsafe {
                let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                tme.cbSize = size_of::<TRACKMOUSEEVENT>() as u32;
                tme.dwFlags = TME_LEAVE;
                tme.hwndTrack = hwnd;
                TrackMouseEvent(&mut tme);
            }
            return true;
        }
        false
    });
    if close_editor_preview {
        exit_editor_mode();
    }
    changed
}

/// 判断路径是否为文本文件（用内置编辑器打开）。
fn is_text_file(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    TEXT_EXTENSIONS.contains(&ext.as_str())
}

// ---------------------------------------------------------------------------
// 输入框子类化 + 输入窗过程
// ---------------------------------------------------------------------------

/// EDIT 子类化：launcher 模式 Enter 提交 / Esc 取消；编辑器模式 Esc 退出编辑、
/// Ctrl+S 保存；其余转发原过程。
unsafe extern "system" fn edit_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_KEYDOWN {
        let (in_editor, is_rename, in_new_note) = SHELL.with(|s| {
            let st = s.borrow();
            (
                st.core.editor.is_some(),
                st.rename_edit == hwnd,
                st.pending_new_note.is_some(),
            )
        });
        if in_editor {
            match wparam as u16 {
                VK_ESCAPE => {
                    exit_editor_mode();
                    return 0;
                }
                // 文件名输入框 Enter = 保存并退出；编辑区 Enter = 换行（走原过程）
                VK_RETURN if is_rename => {
                    exit_editor_mode();
                    return 0;
                }
                _ => {
                    // Ctrl+S 保存
                    if wparam as u16 == b'S' as u16 && unsafe { GetKeyState(VK_CONTROL as i32) } < 0 {
                        save_editor_ui();
                        return 0;
                    }
                }
            }
        } else {
            match wparam as u16 {
                VK_RETURN => {
                    if in_new_note {
                        submit_new_note();
                    } else {
                        submit_input();
                    }
                    return 0;
                }
                VK_ESCAPE => {
                    if in_new_note {
                        cancel_new_note();
                    } else {
                        deactivate_overlay();
                    }
                    return 0;
                }
                _ => {}
            }
        }
    }
    // 编辑器内 hover 联动（skatch 模式）：鼠标所在字符 → 段落 → 卡片指示条
    if msg == WM_MOUSEMOVE || msg == WM_MOUSELEAVE {
        let skatch_mode = SHELL.with(|s| {
            let st = s.borrow();
            hwnd == st.edit_editor
                && st.core
                    .editor
                    .as_ref()
                    .map_or(false, |e| e.skatch_index.is_some())
        });
        if skatch_mode {
            let hit: Option<usize> = if msg == WM_MOUSEMOVE {
                unsafe {
                    // 注册 leave 跟踪（离开编辑区时清除指示）
                    let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
                    tme.cbSize = size_of::<TRACKMOUSEEVENT>() as u32;
                    tme.dwFlags = TME_LEAVE;
                    tme.hwndTrack = hwnd;
                    TrackMouseEvent(&mut tme);
                    let pt = POINT {
                        x: (lparam & 0xffff) as i32 as i16 as i32,
                        y: ((lparam >> 16) & 0xffff) as i32 as i16 as i32,
                    };
                    let cp = SendMessageW(hwnd, EM_CHARFROMPOS, 0, &pt as *const POINT as isize)
                        as usize
                        & 0xffff;
                    SHELL.with(|s| {
                        let st = s.borrow();
                        let content = st.core.editor.as_ref().map(|e| e.content.clone());
                        content.map(|c| {
                            // RichEdit 内部为 CRLF：把内容按显示文本（\n → \r\n）
                            // 转 UTF-16，cp 前的 \r 数量即行号（按行分段的段号）
                            let disp: Vec<u16> = c.replace('\n', "\r\n").encode_utf16().collect();
                            disp[..cp.min(disp.len())]
                                .iter()
                                .filter(|&&u| u == 0x0D)
                                .count()
                        })
                    })
                }
            } else {
                None
            };
            let changed = SHELL.with(|s| {
                let mut sh = s.borrow_mut();
                if sh.nav_hover != hit {
                    sh.nav_hover = hit;
                    true
                } else {
                    false
                }
            });
            if changed {
                let overlay = SHELL.with(|s| s.borrow().hwnd);
                render_overlay(overlay);
            }
        }
    }
    let old = SHELL.with(|s| {
        let st = s.borrow();
        if hwnd == st.edit_editor {
            st.old_edit_editor_proc
        } else {
            st.old_edit_proc
        }
    });
    match old {
        Some(_) => unsafe { CallWindowProcW(old, hwnd, msg, wparam, lparam) },
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 编辑器路径显示：**远程模式**（服务地址非回环）显示 `<host>:<目录内相对路径>`，
/// 本机模式显示完整路径。
fn display_editor_path(path: &str, root: Option<&str>) -> String {
    let host = ipc::server_addr()
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_default();
    let remote = !host.is_empty() && host != "localhost" && host != "127.0.0.1" && host != "::1";
    if remote {
        let rel = root
            .and_then(|r| path.strip_prefix(r))
            .map(|p| p.trim_start_matches(['/', '\\']).to_string())
            .unwrap_or_else(|| path.to_string());
        format!("{host}:{rel}")
    } else {
        path.to_string()
    }
}

/// 编辑器窗口布局（给定窗口尺寸，返回编辑区与三个自绘按钮的矩形）。
///
/// - 头部一行：文件名输入框（左）· 保存（中）· ✕（右）；
/// - 底栏一行：「打开目录」按钮在右端，与路径文字同区。
fn editor_layout(w: i32, h: i32) -> (Rect, Rect, Rect, Rect) {
    // 编辑区：上分隔线之下、下分割线之上（不与路径/按钮功能区重叠）
    let edit = Rect {
        x: 14,
        y: EDITOR_HEADER_H + 4,
        w: w - 28,
        h: h - EDITOR_HEADER_H - 76,
    };
    // 头部尾：保存（中）· ✕（右），与文件名输入框同一行
    let save = Rect {
        x: w - 126,
        y: 8,
        w: 84,
        h: EDITOR_BTN_H,
    };
    let close = Rect {
        x: w - 38,
        y: 8,
        w: 34,
        h: EDITOR_BTN_H,
    };
    // 底栏右端：「打开目录」按钮（与路径文字同底栏，右对齐）
    let location = Rect {
        x: w - 104,
        y: h - 58,
        w: 90,
        h: 28,
    };
    (edit, location, save, close)
}

/// 编辑器编辑区矩形（enter_editor_mode 定位 EDIT 用）。
fn editor_edit_rect(w: i32, h: i32) -> Rect {
    editor_layout(w, h).0
}

/// 编辑器自绘按钮命中测试（0=无，1=打开所在位置，2=保存，3=✕）。
fn editor_button_at(x: i32, y: i32, w: i32, h: i32) -> u8 {
    let (_, location, save, close) = editor_layout(w, h);
    if location.contains(x, y) {
        BTN_LOCATION
    } else if save.contains(x, y) {
        BTN_SAVE
    } else if close.contains(x, y) {
        BTN_CLOSE
    } else {
        0
    }
}

/// 把窗口设为圆角（系统接管 rgn 所有权；窗口尺寸变化后需重新设置）。
fn set_window_rounded(hwnd: HWND, w: i32, h: i32, radius: i32) {
    unsafe {
        let rgn = CreateRoundRectRgn(0, 0, w, h, radius * 2, radius * 2);
        if !rgn.is_null() {
            SetWindowRgn(hwnd, rgn, 1);
        }
    }
}

/// 自绘一个 Web 风格按钮（圆角 + 填充 + 悬停态）。
fn draw_button(hdc: HDC, r: &Rect, text: &str, hovered: bool, primary: bool) {
    unsafe {
        let rect = to_rect(*r);
        let fill = if primary {
            rgb(72, 133, 195)
        } else if hovered {
            rgb(58, 63, 73)
        } else {
            rgb(46, 50, 58)
        };
        let brush = CreateSolidBrush(fill);
        fill_round_rect(hdc, &rect, 6, brush);
        DeleteObject(brush);
        let border = CreateSolidBrush(if primary {
            rgb(40, 92, 145)
        } else {
            rgb(70, 75, 86)
        });
        frame_round_rect(hdc, &rect, 6, border);
        DeleteObject(border);
        let color = if primary {
            rgb(255, 255, 255)
        } else if hovered {
            rgb(235, 238, 245)
        } else {
            rgb(190, 196, 208)
        };
        let font = SHELL.with(|s| s.borrow().card_font);
        draw_text_center(hdc, text, &mut to_rect(*r), color, font);
    }
}

/// 输入框窗口过程：深色圆角主题（launcher 命令框 / 编辑器窗口自绘 UI）。
unsafe extern "system" fn input_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // 自绘：背景 + 边框 + （编辑器模式）头部信息栏与三个按钮
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let dark = SHELL.with(|s| s.borrow().dark_brush);
                FillRect(hdc, &r, dark);

                let in_editor = SHELL.with(|s| s.borrow().core.editor.is_some());
                if in_editor {
                    // 边框
                    let border = CreateSolidBrush(rgb(110, 120, 135));
                    FrameRect(hdc, &r, border);
                    DeleteObject(border);

                    // 头部第一行是文件名输入框（EDIT 子控件，可改名）；
                    // 上分隔线：与卡片一致，完全贯通（左缘到右缘）
                    let line = RECT {
                        left: 0,
                        top: EDITOR_HEADER_H - 1,
                        right: w,
                        bottom: EDITOR_HEADER_H,
                    };
                    let line_brush = CreateSolidBrush(rgb(55, 59, 68));
                    FillRect(hdc, &line, line_brush);
                    DeleteObject(line_brush);

                    // 下分割线：编辑区与功能区（路径 + 按钮）之间，同样贯通
                    let line2 = RECT {
                        left: 0,
                        top: h - 69,
                        right: w,
                        bottom: h - 68,
                    };
                    let line2_brush = CreateSolidBrush(rgb(55, 59, 68));
                    FillRect(hdc, &line2, line2_brush);
                    DeleteObject(line2_brush);

                    // 底栏：路径（弱化小字；远程时显示 <host>:<目录内路径>）
                    let path = SHELL.with(|s| {
                        s.borrow()
                            .core
                            .editor
                            .as_ref()
                            .map(|ed| display_editor_path(&ed.path, ed.root.as_deref()))
                            .unwrap_or_default()
                    });
                    let mut path_r = RECT {
                        left: 18,
                        top: h - 60,
                        right: w - 120,
                        bottom: h - 30,
                    };
                    draw_text(hdc, &path, &mut path_r, rgb(140, 146, 158), SHELL.with(|s| s.borrow().card_font));

                    // 自绘按钮：头部尾 = 保存/✕，底栏右端 = 打开目录
                    let (_, location, save, close) = editor_layout(w, h);
                    let hover = SHELL.with(|s| s.borrow().btn_hover);
                    draw_button(hdc, &location, "打开目录", hover == BTN_LOCATION, false);
                    draw_button(hdc, &save, "保存", hover == BTN_SAVE, true);
                    draw_button(hdc, &close, "✕", hover == BTN_CLOSE, false);
                } else {
                    // launcher 命令框：细边框（青蓝色调，与卡片强调色呼应）
                    let border = CreateSolidBrush(rgb(82, 128, 165));
                    FrameRect(hdc, &r, border);
                    DeleteObject(border);
                }

                EndPaint(hwnd, &ps);
            }
            0
        }
        // 自绘按钮点击（编辑器模式；编辑区由 EDIT 子控件独占）
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let in_editor = SHELL.with(|s| s.borrow().core.editor.is_some());
            if in_editor {
                let mut r: RECT = unsafe { std::mem::zeroed() };
                unsafe { GetClientRect(hwnd, &mut r) };
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                match editor_button_at(x, y, w, h) {
                    BTN_LOCATION => open_editor_location(),
                    BTN_SAVE => save_editor_ui(),
                    BTN_CLOSE => exit_editor_mode(),
                    _ => {}
                }
            }
            0
        }
        // 自绘按钮悬停（编辑器模式）+ skatch 导航条 hover 联动
        WM_MOUSEMOVE => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let in_editor = SHELL.with(|s| s.borrow().core.editor.is_some());
            if in_editor {
                let mut r: RECT = unsafe { std::mem::zeroed() };
                unsafe { GetClientRect(hwnd, &mut r) };
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let hover = editor_button_at(x, y, w, h);
                let changed = SHELL.with(|s| {
                    let mut st = s.borrow_mut();
                    if st.btn_hover != hover {
                        st.btn_hover = hover;
                        true
                    } else {
                        false
                    }
                });
                if changed {
                    unsafe { InvalidateRect(hwnd, null_mut(), 0) };
                }
            }
            0
        }
        WM_MOUSELEAVE => {
            let changed = SHELL.with(|s| {
                let mut st = s.borrow_mut();
                let mut c = false;
                if st.btn_hover != 0 {
                    st.btn_hover = 0;
                    c = true;
                }
                if st.nav_hover.take().is_some() {
                    c = true;
                }
                c
            });
            if changed {
                unsafe { InvalidateRect(hwnd, null_mut(), 0) };
                let overlay = SHELL.with(|s| s.borrow().hwnd);
                render_overlay(overlay);
            }
            0
        }
        // 深色背景（配合 EDIT 的 WM_CTLCOLOREDIT / RichEdit 的 WM_CTLCOLOR）
        WM_ERASEBKGND => {
            let dark = SHELL.with(|s| s.borrow().dark_brush);
            unsafe {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wparam as HDC, &r, dark);
            }
            1
        }
        // EDIT 深色配色：白字 + 深底
        WM_CTLCOLOREDIT => {
            unsafe {
                let hdc = wparam as HDC;
                SetTextColor(hdc, rgb(235, 238, 245));
                SetBkColor(hdc, rgb(40, 43, 50));
            }
            SHELL.with(|s| s.borrow().dark_brush as LRESULT)
        }
        // RichEdit 深色配色（发送 WM_CTLCOLOR 而非 WM_CTLCOLOREDIT）
        WM_CTLCOLOR => {
            unsafe {
                let hdc = wparam as HDC;
                SetTextColor(hdc, rgb(235, 238, 245));
                SetBkColor(hdc, rgb(40, 43, 50));
            }
            SHELL.with(|s| s.borrow().dark_brush as LRESULT)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------
// 设置对话框：配置 anm-core 服务地址（托盘菜单「设置服务地址…」）
// ---------------------------------------------------------------------------

/// 打开设置对话框：居中显示、预填当前服务地址与令牌、聚焦地址输入框。
fn open_settings_dialog() {
    let (hwnd, edit, token_edit, addr, token) = SHELL.with(|s| {
        let mut sh = s.borrow_mut();
        sh.settings_error.clear();
        sh.settings_btn_hover = 0;
        (
            sh.settings_hwnd,
            sh.settings_edit,
            sh.settings_token_edit,
            ipc::server_addr(),
            ipc::server_token().unwrap_or_default(),
        )
    });
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    unsafe {
        set_window_rounded(hwnd, SETTINGS_W, SETTINGS_H, 12);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            (sw - SETTINGS_W) / 2,
            (sh - SETTINGS_H) / 2,
            SETTINGS_W,
            SETTINGS_H,
            SWP_SHOWWINDOW,
        );
        SetWindowPos(
            edit,
            null_mut(),
            18,
            58,
            SETTINGS_W - 36,
            28,
            SWP_SHOWWINDOW,
        );
        SetWindowPos(
            token_edit,
            null_mut(),
            18,
            116,
            SETTINGS_W - 36,
            28,
            SWP_SHOWWINDOW,
        );
        let wide = to_wide(&addr);
        SetWindowTextW(edit, wide.as_ptr());
        let wide2 = to_wide(&token);
        SetWindowTextW(token_edit, wide2.as_ptr());
        ShowWindow(edit, SW_SHOW);
        ShowWindow(token_edit, SW_SHOW);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(edit);
    }
    unsafe { InvalidateRect(hwnd, null_mut(), 0) };
}

/// 确认设置：校验 → 保存配置文件 → 应用覆盖 → 关闭（成功轻提示）。
fn apply_settings() {
    let (hwnd, text, token) = SHELL.with(|s| {
        let st = s.borrow();
        (
            st.settings_hwnd,
            read_edit_text(st.settings_edit),
            read_edit_text(st.settings_token_edit),
        )
    });
    let addr = text.trim().to_string();
    let token = token.trim().to_string();
    if let Err(e) = ipc::validate_addr(&addr) {
        SHELL.with(|s| s.borrow_mut().settings_error = e);
        unsafe { InvalidateRect(hwnd, null_mut(), 0) };
        return;
    }
    let mut cfg = load_tray_config();
    cfg.server_addr = Some(addr.clone());
    cfg.server_token = if token.is_empty() { None } else { Some(token.clone()) };
    save_tray_config(&cfg);
    ipc::set_server_addr_override(Some(addr.clone()));
    ipc::set_server_token_override(if token.is_empty() { None } else { Some(token) });
    unsafe { ShowWindow(hwnd, SW_HIDE) };
    show_toast(format!("服务地址已更新为 {addr}"));
}

/// 取消设置：直接关闭。
fn cancel_settings() {
    let hwnd = SHELL.with(|s| s.borrow().settings_hwnd);
    unsafe { ShowWindow(hwnd, SW_HIDE) };
}

/// 设置对话框布局：按钮矩形（取消左、确定右，Web 惯例主按钮在右）。
fn settings_buttons(w: i32, h: i32) -> (Rect, Rect) {
    let y = h - 46;
    (
        Rect { x: w - 208, y, w: 90, h: 28 },
        Rect { x: w - 108, y, w: 90, h: 28 },
    )
}

/// 设置对话框命中测试（0=无，1=确定，2=取消）。
fn settings_button_at(x: i32, y: i32, w: i32, h: i32) -> u8 {
    let (cancel, ok) = settings_buttons(w, h);
    if ok.contains(x, y) {
        BTN_SETTINGS_OK
    } else if cancel.contains(x, y) {
        BTN_SETTINGS_CANCEL
    } else {
        0
    }
}

/// 设置对话框窗口过程：深色圆角 + 标题/提示/错误 + 自绘按钮。
unsafe extern "system" fn settings_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let dark = SHELL.with(|s| s.borrow().dark_brush);
                FillRect(hdc, &r, dark);

                // 边框
                let border = CreateSolidBrush(rgb(110, 120, 135));
                FrameRect(hdc, &r, border);
                DeleteObject(border);

                let (card_font, input_font) = SHELL.with(|s| (s.borrow().card_font, s.borrow().input_font));

                // 标题
                let mut title_r = RECT { left: 18, top: 12, right: w - 18, bottom: 36 };
                draw_text(hdc, "anm-core 服务设置", &mut title_r, rgb(95, 190, 255), input_font);
                // 服务地址标签
                let mut addr_label_r = RECT { left: 18, top: 34, right: w - 18, bottom: 54 };
                draw_text(
                    hdc,
                    "服务地址（主机:端口，如 192.168.0.102:17370）",
                    &mut addr_label_r,
                    rgb(140, 146, 158),
                    card_font,
                );
                // 服务地址输入框边框（视觉上的输入字段）
                let edit_field = RECT {
                    left: 18,
                    top: 58,
                    right: w - 18,
                    bottom: 86,
                };
                let field_border = CreateSolidBrush(rgb(75, 118, 155));
                frame_round_rect(hdc, &edit_field, 6, field_border);
                DeleteObject(field_border);

                // 访问令牌标签 + 输入框边框
                let mut token_label_r = RECT { left: 18, top: 92, right: w - 18, bottom: 112 };
                draw_text(
                    hdc,
                    "访问令牌（可选，须与服务端 [server] token 一致）",
                    &mut token_label_r,
                    rgb(140, 146, 158),
                    card_font,
                );
                let token_field = RECT {
                    left: 18,
                    top: 116,
                    right: w - 18,
                    bottom: 144,
                };
                let token_border = CreateSolidBrush(rgb(75, 118, 155));
                frame_round_rect(hdc, &token_field, 6, token_border);
                DeleteObject(token_border);

                // 错误（红色）/ 当前生效信息（绿色）
                let err = SHELL.with(|s| s.borrow().settings_error.clone());
                let token_state = if ipc::server_token().is_some() { "已设置" } else { "未设置" };
                let cur = format!("当前生效：{}（令牌：{token_state}）", ipc::server_addr());
                if !err.is_empty() {
                    let mut err_r = RECT { left: 18, top: 152, right: w - 18, bottom: 196 };
                    draw_text_multi(hdc, &err, &mut err_r, rgb(255, 120, 120), card_font);
                } else {
                    let mut cur_r = RECT { left: 18, top: 152, right: w - 18, bottom: 176 };
                    draw_text(hdc, &cur, &mut cur_r, rgb(150, 200, 160), card_font);
                }

                // 按钮（取消 / 确定）
                let (cancel, ok) = settings_buttons(w, h);
                let hover = SHELL.with(|s| s.borrow().settings_btn_hover);
                draw_button(hdc, &cancel, "取消", hover == BTN_SETTINGS_CANCEL, false);
                draw_button(hdc, &ok, "确定", hover == BTN_SETTINGS_OK, true);

                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(hwnd, &mut r) };
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            match settings_button_at(x, y, w, h) {
                BTN_SETTINGS_OK => apply_settings(),
                BTN_SETTINGS_CANCEL => cancel_settings(),
                _ => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xffff) as i32 as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i32 as i16 as i32;
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(hwnd, &mut r) };
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let hover = settings_button_at(x, y, w, h);
            let changed = SHELL.with(|s| {
                let mut st = s.borrow_mut();
                if st.settings_btn_hover != hover {
                    st.settings_btn_hover = hover;
                    true
                } else {
                    false
                }
            });
            if changed {
                unsafe { InvalidateRect(hwnd, null_mut(), 0) };
            }
            0
        }
        WM_MOUSELEAVE => {
            let changed = SHELL.with(|s| {
                let mut st = s.borrow_mut();
                if st.settings_btn_hover != 0 {
                    st.settings_btn_hover = 0;
                    true
                } else {
                    false
                }
            });
            if changed {
                unsafe { InvalidateRect(hwnd, null_mut(), 0) };
            }
            0
        }
        WM_ERASEBKGND => {
            let dark = SHELL.with(|s| s.borrow().dark_brush);
            unsafe {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wparam as HDC, &r, dark);
            }
            1
        }
        WM_CTLCOLOREDIT => {
            unsafe {
                let hdc = wparam as HDC;
                SetTextColor(hdc, rgb(235, 238, 245));
                SetBkColor(hdc, rgb(40, 43, 50));
            }
            SHELL.with(|s| s.borrow().dark_brush as LRESULT)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// 设置输入框子类化：Enter = 确定，Esc = 取消。
unsafe extern "system" fn settings_edit_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_KEYDOWN {
        match wparam as u16 {
            VK_RETURN => {
                apply_settings();
                return 0;
            }
            VK_ESCAPE => {
                cancel_settings();
                return 0;
            }
            _ => {}
        }
    }
    let old = SHELL.with(|s| {
        let st = s.borrow();
        if hwnd == st.settings_edit {
            st.old_settings_edit_proc
        } else {
            st.old_settings_token_edit_proc
        }
    });
    match old {
        Some(_) => unsafe { CallWindowProcW(old, hwnd, msg, wparam, lparam) },
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------
// 全局快捷键设置对话框：按下新组合 → 回车确认 / Esc 取消
// ---------------------------------------------------------------------------

/// 注册/更换全局快捷键：先注销旧的（未注册时忽略失败），再注册新的。
/// 返回是否注册成功。
fn register_hotkey(hwnd: HWND, hk: &Hotkey) -> bool {
    unsafe {
        UnregisterHotKey(hwnd, HOTKEY_ID);
        RegisterHotKey(hwnd, HOTKEY_ID, hk.mods, hk.vk) != 0
    }
}

/// 打开快捷键设置对话框：先临时注销全局快捷键（避免按下当前组合时触发
/// 切换而非被捕获），关闭/应用时再恢复注册。
fn open_hotkey_dialog() {
    let (dlg, tray, cur) = SHELL.with(|s| {
        let st = s.borrow();
        (
            st.hotkey_hwnd,
            st.tray_hwnd,
            st.hotkey
                .map(|h| hotkey::format(&h))
                .unwrap_or_else(|| "未注册".to_string()),
        )
    });
    unsafe {
        UnregisterHotKey(tray, HOTKEY_ID);
        SHELL.with(|s| {
            let mut sh = s.borrow_mut();
            sh.hotkey_hint = format!("请按下新的快捷键组合…（当前：{cur}）");
            sh.hotkey_pending = None;
        });
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        set_window_rounded(dlg, HOTKEY_W, HOTKEY_H, 12);
        SetWindowPos(
            dlg,
            HWND_TOPMOST,
            (sw - HOTKEY_W) / 2,
            (sh - HOTKEY_H) / 2,
            HOTKEY_W,
            HOTKEY_H,
            SWP_SHOWWINDOW,
        );
        ShowWindow(dlg, SW_SHOW);
        SetForegroundWindow(dlg);
        SetFocus(dlg);
        InvalidateRect(dlg, null_mut(), 0);
    }
}

/// 关闭快捷键对话框（不应用）：恢复旧快捷键的注册。
fn close_hotkey_dialog() {
    let (dlg, tray) = SHELL.with(|s| (s.borrow().hotkey_hwnd, s.borrow().tray_hwnd));
    unsafe {
        ShowWindow(dlg, SW_HIDE);
        if let Some(hk) = SHELL.with(|s| s.borrow().hotkey) {
            register_hotkey(tray, &hk);
        }
    }
}

/// 应用待确认的快捷键：注册新组合 → 持久化；失败恢复旧组合并提示。
fn apply_hotkey() {
    let pending = SHELL.with(|s| s.borrow_mut().hotkey_pending.take());
    let Some(hk) = pending else {
        close_hotkey_dialog();
        return;
    };
    let (dlg, tray) = SHELL.with(|s| (s.borrow().hotkey_hwnd, s.borrow().tray_hwnd));
    let desc = hotkey::format(&hk);
    if register_hotkey(tray, &hk) {
        SHELL.with(|s| s.borrow_mut().hotkey = Some(hk));
        let mut cfg = load_tray_config();
        cfg.hotkey = Some(desc.clone());
        save_tray_config(&cfg);
        unsafe { ShowWindow(dlg, SW_HIDE) };
        show_toast(format!("快捷键已更新为 {desc}"));
    } else {
        // 新组合被系统/其他程序占用：恢复旧组合，对话框内提示
        let old = SHELL.with(|s| s.borrow().hotkey);
        if let Some(old) = old {
            register_hotkey(tray, &old);
        }
        SHELL.with(|s| s.borrow_mut().hotkey_hint = format!("{desc} 注册失败（可能被其他程序占用）"));
        unsafe { InvalidateRect(dlg, null_mut(), 0) };
    }
}

/// 快捷键设置对话框窗口过程：捕获按键组合、回车确认、Esc 取消。
unsafe extern "system" fn hotkey_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam as u32;
            // 修饰键位（Ctrl/Alt/Shift/Win 按下即计入）
            let mods = (if unsafe { GetKeyState(VK_CONTROL as i32) } < 0 { MOD_CONTROL } else { 0 })
                | (if unsafe { GetKeyState(VK_MENU as i32) } < 0 { MOD_ALT } else { 0 })
                | (if unsafe { GetKeyState(VK_SHIFT as i32) } < 0 { MOD_SHIFT } else { 0 })
                | (if unsafe { GetKeyState(VK_LWIN as i32) } < 0
                    || unsafe { GetKeyState(VK_RWIN as i32) } < 0
                {
                    MOD_WIN
                } else {
                    0
                });
            // 无修饰键时：回车 = 确认，Esc = 取消
            if mods == 0 {
                if vk == VK_RETURN as u32 {
                    apply_hotkey();
                    return 0;
                }
                if vk == VK_ESCAPE as u32 {
                    close_hotkey_dialog();
                    return 0;
                }
            }
            // 纯修饰键本身忽略
            let is_modifier = vk == VK_CONTROL as u32
                || vk == VK_MENU as u32
                || vk == VK_SHIFT as u32
                || vk == VK_LWIN as u32
                || vk == VK_RWIN as u32;
            if is_modifier {
                return 0;
            }
            // 组合键：捕获显示，等待回车确认
            if let Some(hk) = Hotkey::new(mods, vk) {
                let desc = hotkey::format(&hk);
                SHELL.with(|s| {
                    let mut sh = s.borrow_mut();
                    sh.hotkey_pending = Some(hk);
                    sh.hotkey_hint = format!("已捕获：{desc}　回车确认 · Esc 取消");
                });
                unsafe { InvalidateRect(hwnd, null_mut(), 0) };
            }
            0
        }
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let (w, _h) = (r.right - r.left, r.bottom - r.top);
                let dark = SHELL.with(|s| s.borrow().dark_brush);
                FillRect(hdc, &r, dark);
                let border = CreateSolidBrush(rgb(110, 120, 135));
                FrameRect(hdc, &r, border);
                DeleteObject(border);
                let (card_font, input_font) =
                    SHELL.with(|s| (s.borrow().card_font, s.borrow().input_font));

                let mut title_r = RECT { left: 18, top: 14, right: w - 18, bottom: 38 };
                draw_text(hdc, "设置全局快捷键", &mut title_r, rgb(95, 190, 255), input_font);

                let hint = SHELL.with(|s| s.borrow().hotkey_hint.clone());
                let mut hint_r = RECT { left: 18, top: 70, right: w - 18, bottom: 104 };
                draw_text(hdc, &hint, &mut hint_r, rgb(235, 238, 245), input_font);

                let mut tip_r = RECT { left: 18, top: 120, right: w - 18, bottom: 144 };
                draw_text(hdc, "按下新的组合（需含 Ctrl/Alt/Shift/Win）· 回车确认 · Esc 取消", &mut tip_r, rgb(140, 146, 158), card_font);

                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_ERASEBKGND => {
            let dark = SHELL.with(|s| s.borrow().dark_brush);
            unsafe {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wparam as HDC, &r, dark);
            }
            1
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------
// 轻提示（toast）：右上角小窗，自动隐藏
// ---------------------------------------------------------------------------

/// 显示轻提示（3 秒后自动隐藏；重复调用会重置计时并覆盖文本）。
fn show_toast(text: String) {
    let (hwnd, sw, _sh) = SHELL.with(|s| {
        let st = s.borrow();
        (st.toast_hwnd, unsafe { GetSystemMetrics(SM_CXSCREEN) }, unsafe {
            GetSystemMetrics(SM_CYSCREEN)
        })
    });
    SHELL.with(|s| s.borrow_mut().toast_text = text);
    unsafe {
        set_window_rounded(hwnd, TOAST_W, TOAST_H, 10);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            sw - TOAST_W - 24,
            24,
            TOAST_W,
            TOAST_H,
            SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOW);
        InvalidateRect(hwnd, null_mut(), 0);
        SetTimer(hwnd, TOAST_TIMER as usize, TOAST_MS as u32, None);
    }
}

/// 轻提示窗口过程：深色圆角小窗 + 文本；计时器到点或点击后隐藏。
unsafe extern "system" fn toast_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let (w, h) = (r.right - r.left, r.bottom - r.top);
                let dark = SHELL.with(|s| s.borrow().dark_brush);
                FillRect(hdc, &r, dark);
                let border = CreateSolidBrush(rgb(90, 130, 165));
                FrameRect(hdc, &r, border);
                DeleteObject(border);
                // 左侧状态色点
                let dot = RECT {
                    left: 16,
                    top: (h - 8) / 2,
                    right: 24,
                    bottom: (h - 8) / 2 + 8,
                };
                let dot_brush = CreateSolidBrush(rgb(110, 200, 150));
                fill_round_rect(hdc, &dot, 4, dot_brush);
                DeleteObject(dot_brush);
                let text = SHELL.with(|s| s.borrow().toast_text.clone());
                let mut text_r = RECT {
                    left: 34,
                    top: 0,
                    right: w - 14,
                    bottom: h,
                };
                let font = SHELL.with(|s| s.borrow().card_font);
                draw_text(hdc, &text, &mut text_r, rgb(235, 238, 245), font);
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_TIMER if wparam == TOAST_TIMER => {
            unsafe {
                KillTimer(hwnd, TOAST_TIMER);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        // 点击立即关闭
        WM_LBUTTONDOWN => {
            unsafe {
                KillTimer(hwnd, TOAST_TIMER);
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_ERASEBKGND => {
            let dark = SHELL.with(|s| s.borrow().dark_brush);
            unsafe {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wparam as HDC, &r, dark);
            }
            1
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ---------------------------------------------------------------------------
// 托盘配置持久化（%APPDATA%/anm-tray-win/config.json）
// ---------------------------------------------------------------------------

/// 托盘配置文件路径。
fn tray_config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join(LAYOUT_DIR).join("config.json"))
}

/// 托盘持久化配置：服务地址 / 访问令牌 / 全局快捷键（字符串形式）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct TrayConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    server_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hotkey: Option<String>,
}

/// 读取持久化配置；文件不存在/损坏/字段非法时逐项回落默认值。
/// 服务地址做格式校验（防手工编辑混入乱码）；快捷键交给 hotkey::parse 校验。
fn load_tray_config() -> TrayConfig {
    let mut cfg = TrayConfig::default();
    let Some(path) = tray_config_path() else {
        return cfg;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return cfg;
    };
    let Ok(v) = serde_json::from_str::<TrayConfig>(&text) else {
        return cfg;
    };
    if let Some(addr) = v.server_addr {
        if ipc::validate_addr(&addr).is_ok() {
            cfg.server_addr = Some(addr);
        }
    }
    if let Some(token) = v.server_token {
        if !token.trim().is_empty() {
            cfg.server_token = Some(token);
        }
    }
    if let Some(hk) = v.hotkey {
        if hotkey::parse(&hk).is_some() {
            cfg.hotkey = Some(hk);
        }
    }
    cfg
}

/// 保存配置到文件（合并现有：只覆盖非 None 字段）。
fn save_tray_config(cfg: &TrayConfig) {
    let Some(path) = tray_config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, text);
    }
}

// ---------------------------------------------------------------------------
// 渲染（逐像素合成）
// ---------------------------------------------------------------------------

/// 逐像素合成渲染变暗层：背景半透明黑 + 卡片/文字完全不透明。
fn render_overlay(hwnd: HWND) {
    unsafe {
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = sw;
        bmi.bmiHeader.biHeight = -sh;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let dib = CreateDIBSection(null_mut(), &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            return;
        }
        let memdc = CreateCompatibleDC(null_mut());
        let old: HGDIOBJ = SelectObject(memdc, dib as HGDIOBJ);

        draw_overlay_content(memdc, sw, sh);
        fix_alpha(bits as *mut u32, (sw as usize) * (sh as usize));

        let size = SIZE { cx: sw, cy: sh };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            null_mut(),
            null_mut(),
            &size,
            memdc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        );

        SelectObject(memdc, old);
        DeleteDC(memdc);
        DeleteObject(dib as _);
    }
}

/// 把覆盖层内容画进给定 DC：纯黑背景 + 卡片（Web 风格：圆角、调色板强调条、
/// 渐变标题带、圆角悬停、细圆角滚动条）。
fn draw_overlay_content(hdc: HDC, sw: i32, sh: i32) {
    unsafe {
        let client = RECT {
            left: 0,
            top: 0,
            right: sw,
            bottom: sh,
        };
        SHELL.with(|s| {
            let st = s.borrow();
            let params = &st.params;
            let bg = CreateSolidBrush(rgb(0, 0, 0));
            FillRect(hdc, &client, bg);
            DeleteObject(bg);

            // 文件拖动中：鼠标下的卡片作为移动目标（高亮描边）
            let drag_target = if let Some(rp) = &st.row_press {
                if rp.moved && rp.path.is_some() {
                    let mut pt: POINT = std::mem::zeroed();
                    GetCursorPos(&mut pt);
                    cards::hit_test(pt.x, pt.y, &st.core.cards).map(|h| h.card)
                } else {
                    None
                }
            } else {
                None
            };
            // 绘制顺序：未置顶的卡片保持原序在前；置顶过的卡片按
            // hover 先后排在后面（最近 hover 的在最上层）。hover 卡片
            // 天然在置顶层中（hover 时已入 top），移开后仍保持置顶。
            let order: Vec<usize> = {
                let mut order = Vec::with_capacity(st.core.cards.len());
                let mut top_idx: Vec<usize> = Vec::new();
                for (i, c) in st.core.cards.iter().enumerate() {
                    if st.core.top.iter().any(|t| *t == c.dir_path) {
                        top_idx.push(i);
                    } else {
                        order.push(i);
                    }
                }
                // top 按 dir_path 顺序重排（保持 top 的先后语义）
                let mut by_top: Vec<usize> = Vec::new();
                for t in &st.core.top {
                    if let Some(pos) = st.core.cards.iter().position(|c| &c.dir_path == t) {
                        by_top.push(pos);
                    }
                }
                for i in top_idx {
                    if !by_top.contains(&i) {
                        by_top.push(i);
                    }
                }
                order.extend(by_top);
                // 子卡置顶：无条件最上层（保持到父/子卡都不再 hover）
                if let Some(sub) = &st.core.subcard_top {
                    if let Some(pos) = order.iter().position(|&i| st.core.cards[i].dir_path == *sub) {
                        let i = order.remove(pos);
                        order.push(i);
                    }
                }
                order
            };
            // 加号按钮 hover：鼠标在加号矩形内（独立判定，与行 hover 互不干预）
            let mouse = || {
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                (pt.x, pt.y)
            };

            let hover_card = st.core.hover.map(|h| h.card);
            for &ci in &order {
                let card = &st.core.cards[ci];
                // 每张卡片从调色板取一个强调色（临时子卡片同主卡片）；
                // skatch 卡片用暖金色强调（与其它卡片区分）
                let accent = if card.skatch {
                    rgb(245, 200, 110)
                } else {
                    ACCENT_PALETTE[ci % ACCENT_PALETTE.len()]
                };
                // 任意行 hover：整卡略微高亮（强度为旧版 1/3）
                let card_hover = hover_card == Some(ci);
                let card_rect = to_rect(card.rect);

                // 1) 卡片阴影（右下 2px 深色错位，模拟投影层次）
                let shadow_rect = RECT {
                    left: card_rect.left + 2,
                    top: card_rect.top + 3,
                    right: card_rect.right + 2,
                    bottom: card_rect.bottom + 3,
                };
                let shadow = CreateSolidBrush(rgb(10, 10, 14));
                fill_round_rect(hdc, &shadow_rect, 10, shadow);
                DeleteObject(shadow);

                // 2) 卡片主体（圆角深色；临时子卡片同主卡片；skatch 明显偏蓝）
                let body = if card.skatch { rgb(27, 37, 54) } else { rgb(38, 41, 48) };
                let body_brush = CreateSolidBrush(body);
                fill_round_rect(hdc, &card_rect, 10, body_brush);
                DeleteObject(body_brush);

                // 3) 左侧强调条（圆角条，Web 风格彩色标签）
                let strip = RECT {
                    left: card_rect.left + 4,
                    top: card_rect.top + 12,
                    right: card_rect.left + 8,
                    bottom: card_rect.bottom - 12,
                };
                let strip_brush = CreateSolidBrush(accent);
                fill_round_rect(hdc, &strip, 2, strip_brush);
                DeleteObject(strip_brush);

                // 4) 标题带（略亮的渐变带：逐行插值，顶部亮底部暗）
                let header_h = params.header_h;
                for i in 0..header_h {
                    let t = i as f32 / header_h as f32;
                    let c = lerp_color(rgb(52, 56, 65), rgb(40, 43, 50), t);
                    let line = RECT {
                        left: card_rect.left + 12,
                        top: card_rect.top + 8 + i,
                        right: card_rect.right - 12,
                        bottom: card_rect.top + 9 + i,
                    };
                    let lbrush = CreateSolidBrush(c);
                    FillRect(hdc, &line, lbrush);
                    DeleteObject(lbrush);
                }

                // 5) 边框（圆角描边；临时子卡片同主卡片；skatch 偏蓝；
                //    整卡 hover 提亮 1/3；文件拖动悬停的目标卡片用强调色描边）
                let border = CreateSolidBrush(if card_hover {
                    rgb(84, 90, 102)
                } else if drag_target == Some(ci) {
                    accent
                } else if card.skatch {
                    rgb(74, 84, 108)
                } else {
                    rgb(68, 72, 82)
                });
                frame_round_rect(hdc, &card_rect, 10, border);
                DeleteObject(border);

                // 6) 可见行
                for (ri, row_rect) in card.row_rects.iter().enumerate() {
                    let mut r = to_rect(*row_rect);
                    r.left += 16;
                    r.right -= 8;
                    let is_hover = st.core.hover == Some(Hit { card: ci, row: ri });
                    // 标题行 hover 不用深色条（整卡高亮替代）；内容行保持贯通阴影条
                    if is_hover && card.row_of(ri) != 0 {
                        // 悬停：左右贯通的直角阴影条（右缘给滚动条留 8px）
                        let hover_rect = RECT {
                            left: card_rect.left + 1,
                            top: row_rect.y + 1,
                            right: card_rect.right - 8,
                            bottom: row_rect.y + row_rect.h - 1,
                        };
                        let hover_brush = CreateSolidBrush(rgb(56, 60, 70));
                        FillRect(hdc, &hover_rect, hover_brush);
                        DeleteObject(hover_brush);
                    }
                    let real = card.row_of(ri);
                    match &card.rows[real] {
                        CardRow::DirHeader => {
                            // 标题：强调色 + 小圆点（内容整体上移 padding/2，
                            // 垂直居中于"边框顶 → 分隔线"区间）
                            let dot = RECT {
                                left: card_rect.left + 13,
                                top: row_rect.y + (row_rect.h - 6) / 2 - params.padding / 2,
                                right: card_rect.left + 19,
                                bottom: row_rect.y + (row_rect.h - 6) / 2 + 6 - params.padding / 2,
                            };
                            let dot_brush = CreateSolidBrush(accent);
                            fill_round_rect(hdc, &dot, 3, dot_brush);
                            DeleteObject(dot_brush);
                            let mut title_r = to_rect(*row_rect);
                            title_r.left += 26;
                            title_r.right -= 36; // 给加号按钮留空间
                            title_r.top -= params.padding / 2;
                            title_r.bottom -= params.padding / 2;
                            draw_text(hdc, &card.title, &mut title_r, accent, st.card_font);
                            // 分隔线：完全分割（顶到卡片左右缘）
                            let line = RECT {
                                left: card_rect.left,
                                top: row_rect.y + row_rect.h - 2,
                                right: card_rect.right,
                                bottom: row_rect.y + row_rect.h - 1,
                            };
                            let line_brush = CreateSolidBrush(rgb(52, 55, 63));
                            FillRect(hdc, &line, line_brush);
                            DeleteObject(line_brush);
                        }
                        CardRow::SubDir { name } => {
                            let label = format!("▸ {name}");
                            draw_text(hdc, &label, &mut r, rgb(105, 205, 150), st.card_font);
                        }
                        CardRow::File { title, .. } => {
                            // skatch 卡片：编辑器导航条 hover 的对应段落行画指示条
                            let nav_idx = st.nav_hover;
                            if card.skatch
                                && nav_idx == Some(real.saturating_sub(1))
                            {
                                let ind = RECT {
                                    left: card_rect.left + 1,
                                    top: row_rect.y + 1,
                                    right: card_rect.right - 8,
                                    bottom: row_rect.y + row_rect.h - 1,
                                };
                                let ind_brush = CreateSolidBrush(rgb(52, 74, 100));
                                FillRect(hdc, &ind, ind_brush);
                                DeleteObject(ind_brush);
                            }
                            // 文件行：小方点 + 弱化文字
                            let dot = RECT {
                                left: card_rect.left + 14,
                                top: row_rect.y + (row_rect.h - 4) / 2,
                                right: card_rect.left + 18,
                                bottom: row_rect.y + (row_rect.h - 4) / 2 + 4,
                            };
                            let dot_brush = CreateSolidBrush(rgb(95, 100, 112));
                            fill_round_rect(hdc, &dot, 2, dot_brush);
                            DeleteObject(dot_brush);
                            let mut file_r = to_rect(*row_rect);
                            file_r.left += 26;
                            draw_text(hdc, title, &mut file_r, rgb(205, 209, 218), st.card_font);
                        }
                    }
                }

                // 7) 卡片右上角「新建笔记」加号（位置固定，滚动后仍可见；
                //    hover 独立判定：鼠标在加号矩形内才亮）
                if let Some(plus) = cards::title_plus_rect(card, params) {
                    let (mx, my) = mouse();
                    let plus_hover = plus.contains(mx, my);
                    let plus_rect = to_rect(plus);
                    let plus_brush = CreateSolidBrush(if plus_hover {
                        rgb(62, 68, 80)
                    } else {
                        rgb(46, 50, 58)
                    });
                    fill_round_rect(hdc, &plus_rect, 5, plus_brush);
                    DeleteObject(plus_brush);
                    let plus_border = CreateSolidBrush(if plus_hover {
                        rgb(110, 120, 135)
                    } else {
                        rgb(70, 75, 86)
                    });
                    frame_round_rect(hdc, &plus_rect, 5, plus_border);
                    DeleteObject(plus_border);
                    // + 号两笔（垂直 + 水平），白色
                    let cx = plus.x + plus.w / 2;
                    let cy = plus.y + plus.h / 2;
                    let v = RECT {
                        left: cx - 1,
                        top: cy - 4,
                        right: cx + 2,
                        bottom: cy + 5,
                    };
                    let h = RECT {
                        left: cx - 4,
                        top: cy - 1,
                        right: cx + 5,
                        bottom: cy + 2,
                    };
                    let plus_pen = CreateSolidBrush(rgb(210, 216, 226));
                    FillRect(hdc, &v, plus_pen);
                    FillRect(hdc, &h, plus_pen);
                    DeleteObject(plus_pen);
                }

                // 8) 细圆角滚动条
                if let (Some(bar), Some(thumb)) = (
                    cards::scrollbar_rect(card, params),
                    cards::scrollbar_thumb(card, params),
                ) {
                    let bar_brush = CreateSolidBrush(rgb(45, 48, 56));
                    fill_round_rect(hdc, &to_rect(bar), 2, bar_brush);
                    DeleteObject(bar_brush);
                    let thumb_brush = CreateSolidBrush(rgb(125, 131, 143));
                    fill_round_rect(hdc, &to_rect(thumb), 2, thumb_brush);
                    DeleteObject(thumb_brush);
                }
            }

            // 右上角状态信息：连接状态（最近一次 IPC 成败）+ 服务地址 + 快捷键
            let ok = ipc::last_ok();
            let addr = ipc::server_addr();
            let hotkey_desc = st
                .hotkey
                .map(|h| hotkey::format(&h))
                .unwrap_or_else(|| "未注册".to_string());
            let (dot_color, state_text) = if ok {
                (rgb(110, 200, 150), "已连接".to_string())
            } else {
                (rgb(255, 120, 120), "未连接".to_string())
            };
            let dot = RECT {
                left: sw - 316,
                top: 20,
                right: sw - 308,
                bottom: 28,
            };
            let dot_brush = CreateSolidBrush(dot_color);
            fill_round_rect(hdc, &dot, 4, dot_brush);
            DeleteObject(dot_brush);
            let mut status_r = RECT {
                left: sw - 300,
                top: 12,
                right: sw - 16,
                bottom: 32,
            };
            draw_text(
                hdc,
                &format!("{state_text} · {addr}"),
                &mut status_r,
                dot_color,
                st.card_font,
            );
            let mut hotkey_r = RECT {
                left: sw - 300,
                top: 32,
                right: sw - 16,
                bottom: 50,
            };
            draw_text(
                hdc,
                &format!("快捷键 {hotkey_desc}"),
                &mut hotkey_r,
                rgb(140, 146, 158),
                st.card_font,
            );

            // 输入框下方的结果 / 错误文本（编辑器模式下隐藏）
            if st.core.editor.is_none() && (!st.core.result.is_empty() || !st.core.error.is_empty())
            {
                let text = if !st.core.error.is_empty() {
                    &st.core.error
                } else {
                    &st.core.result
                };
                let mut r = RECT {
                    left: st.core.input_rect.x,
                    top: st.core.input_rect.y + st.core.input_rect.h + 12,
                    right: st.core.input_rect.x + st.core.input_rect.w,
                    bottom: st.core.input_rect.y + st.core.input_rect.h + 160,
                };
                let color = if !st.core.error.is_empty() {
                    rgb(255, 120, 120)
                } else {
                    rgb(255, 230, 150)
                };
                draw_text_multi(hdc, text, &mut r, color, st.card_font);
            }
        });
    }
}

/// 卡片强调色调色板（按卡片下标循环取色）。
const ACCENT_PALETTE: [u32; 6] = [
    rgb(79, 193, 255),   // 蓝
    rgb(110, 200, 150),  // 绿
    rgb(175, 135, 225),  // 紫
    rgb(240, 175, 95),   // 橙
    rgb(85, 195, 195),   // 青
    rgb(230, 135, 165),  // 粉
];

/// 颜色线性插值（t: 0→1，从 a 到 b）。
fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let ar = (a & 0xff) as f32;
    let ag = ((a >> 8) & 0xff) as f32;
    let ab = ((a >> 16) & 0xff) as f32;
    let br = (b & 0xff) as f32;
    let bg = ((b >> 8) & 0xff) as f32;
    let bb = ((b >> 16) & 0xff) as f32;
    let r = ar + (br - ar) * t;
    let g = ag + (bg - ag) * t;
    let bl = ab + (bb - ab) * t;
    rgb(r as u8, g as u8, bl as u8)
}

/// 圆角矩形填充。
fn fill_round_rect(hdc: HDC, r: &RECT, radius: i32, brush: HBRUSH) {
    unsafe {
        let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        if !rgn.is_null() {
            FillRgn(hdc, rgn, brush);
            DeleteObject(rgn as _);
        }
    }
}

/// 圆角矩形描边。
fn frame_round_rect(hdc: HDC, r: &RECT, radius: i32, brush: HBRUSH) {
    unsafe {
        let old = SelectObject(hdc, brush as HGDIOBJ);
        RoundRect(
            hdc,
            r.left,
            r.top,
            r.right,
            r.bottom,
            radius * 2,
            radius * 2,
        );
        SelectObject(hdc, old);
    }
}

/// alpha 修正：纯黑像素 = 变暗背景 → DIM_ALPHA；其余（卡片/文字）不透明。
fn fix_alpha(bits: *mut u32, len: usize) {
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits, len) };
    for p in pixels.iter_mut() {
        let rgb = *p & 0x00ff_ffff;
        let a = if rgb == 0 { DIM_ALPHA } else { 255 };
        *p = rgb | ((a as u32) << 24);
    }
}

/// 把布局矩形转成 Win32 RECT。
fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

/// 构造 COLORREF（windows-sys 0.59 不导出 GDI 的 RGB 宏，按位拼装）。
const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

/// 单行文本绘制（左对齐垂直居中，超长省略号，不解析 & 前缀）。
fn draw_text(hdc: HDC, text: &str, rect: &mut RECT, color: u32, font: HFONT) {
    unsafe {
        let wide = to_wide(text);
        SetTextColor(hdc, color);
        SetBkMode(hdc, TRANSPARENT as i32);
        let old: HGDIOBJ = SelectObject(hdc, font as HGDIOBJ);
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            rect,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
}

/// 居中文本绘制（自绘按钮文字用）。
fn draw_text_center(hdc: HDC, text: &str, rect: &mut RECT, color: u32, font: HFONT) {
    unsafe {
        let wide = to_wide(text);
        SetTextColor(hdc, color);
        SetBkMode(hdc, TRANSPARENT as i32);
        let old: HGDIOBJ = SelectObject(hdc, font as HGDIOBJ);
        DrawTextW(
            hdc,
            wide.as_ptr(),
            -1,
            rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
}

/// 多行文本绘制（自动换行，用于命令结果）。
fn draw_text_multi(hdc: HDC, text: &str, rect: &mut RECT, color: u32, font: HFONT) {
    unsafe {
        let wide = to_wide(text);
        SetTextColor(hdc, color);
        SetBkMode(hdc, TRANSPARENT as i32);
        let old: HGDIOBJ = SelectObject(hdc, font as HGDIOBJ);
        DrawTextW(hdc, wide.as_ptr(), -1, rect, DT_LEFT | DT_TOP | DT_WORDBREAK | DT_NOPREFIX);
        SelectObject(hdc, old);
    }
}

/// 用系统默认方式打开一个路径（WSL 路径先转换为 Windows 路径）。
///
/// 带 `SEE_MASK_FLAG_NO_UI`：打不开时不弹系统错误框（比如路径只在服务端
/// 机器上），由调用方决定提示方式。返回是否成功。
fn open_with_default_handler(path: &str) -> bool {
    unsafe {
        let win_path = wslpath::to_windows(path);
        let wide = to_wide(&win_path);
        let mut sei: SHELLEXECUTEINFOW = std::mem::zeroed();
        sei.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
        sei.fMask = SEE_MASK_FLAG_NO_UI;
        sei.lpVerb = w!("open");
        sei.lpFile = wide.as_ptr();
        sei.nShow = SW_SHOWNORMAL as i32;
        ShellExecuteExW(&mut sei) != 0
    }
}

// ---------------------------------------------------------------------------
// 位置记忆
// ---------------------------------------------------------------------------

/// 卡片位置记忆文件路径：`%APPDATA%/anm-tray-win/layout.json`。
fn layout_file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join(LAYOUT_DIR).join(LAYOUT_FILE))
}

/// 加载已保存的卡片位置（目录名 → 左上角坐标）；文件不存在/损坏时返回空表。
fn load_positions() -> HashMap<String, (i32, i32)> {
    let Some(path) = layout_file_path() else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str::<HashMap<String, [i32; 2]>>(&text)
        .ok()
        .map(|m| m.into_iter().map(|(k, v)| (k, (v[0], v[1]))).collect())
        .unwrap_or_default()
}

/// 保存卡片位置（不含临时卡片）并同步内存位置表。
fn save_positions() {
    let Some(path) = layout_file_path() else {
        return;
    };
    let map: HashMap<String, [i32; 2]> = SHELL.with(|s| {
        s.borrow()
            .core
            .cards
            .iter()
            .filter(|c| !c.temp)
            .map(|c| (c.title.clone(), [c.rect.x, c.rect.y]))
            .collect()
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&map) {
        let _ = std::fs::write(&path, text);
    }
    let mem_map: HashMap<String, (i32, i32)> = map
        .into_iter()
        .map(|(k, v)| (k, (v[0], v[1])))
        .collect();
    SHELL.with(|s| s.borrow_mut().core.positions = mem_map);
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 把 UTF-16 字符串拷贝进固定长度数组（托盘提示文本用，截断补零）。
fn copy_wide_into(src: &[u16], dst: &mut [u16]) {
    let n = src.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
    dst[n] = 0;
}

/// Rust 字符串 → 以 0 结尾的 UTF-16 缓冲。
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl ShellState {
    /// 占位构造（run() 里立即填充真实句柄与状态）。
    fn placeholder() -> Self {
        Self {
            tray_hwnd: null_mut(),
            hwnd: null_mut(),
            input_hwnd: null_mut(),
            edit: null_mut(),
            edit_editor: null_mut(),
            rename_edit: null_mut(),
            old_rename_edit_proc: None,
            old_edit_editor_proc: None,
            dark_brush: null_mut(),
            btn_hover: 0,
            settings_hwnd: null_mut(),
            settings_edit: null_mut(),
            settings_token_edit: null_mut(),
            old_settings_edit_proc: None,
            old_settings_token_edit_proc: None,
            settings_btn_hover: 0,
            settings_error: String::new(),
            hotkey_hwnd: null_mut(),
            hotkey_hint: String::new(),
            hotkey_pending: None,
            toast_hwnd: null_mut(),
            toast_text: String::new(),
            old_edit_proc: None,
            card_font: null_mut(),
            input_font: null_mut(),
            params: LayoutParams::default(),
            core: TrayState::default(),
            hotkey: None,
            overlay_visible: false,
            pending_new_note: None,
            row_press: None,
            nav_hover: None,
        }
    }
}
