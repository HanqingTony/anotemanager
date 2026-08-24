//! anm-native-ext：anm 托盘（Neutralino 前端）的原生扩展进程（Rust）。
//!
//! 由 Neutralino 壳加载：壳经 stdin 注入连接配置，扩展建立 WebSocket 与壳通信。
//!
//! **架构要点**：Neutralino 壳的"主动事件推送"通道在透明窗口场景不可靠
//! （事件延迟到窗口重绘才到达），而"请求-响应"通道（前端 dispatch → 响应）
//! 稳定。因此本扩展把**全部系统事件下沉到自身**：
//!
//! - 系统消息循环线程：全局热键（RegisterHotKey）+ 系统托盘（Shell_NotifyIcon
//!   + 右键菜单）→ 事件写入共享队列；
//! - 前端**轮询** `poll` 取事件（可靠的响应通道）；
//! - `winmode`：窗口显示/隐藏/置顶（FindWindow + ShowWindow + SetForegroundWindow，
//!   不经壳的窗口 API——壳的透明窗口窗口控制不稳）；
//! - `ipc`：TCP 转发到 anm-core（信封与 anm_core::protocol 一致）。

#[cfg(windows)]
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};

/// 诊断日志（exe 同目录 ext.log）
fn ext_log(msg: &str) {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| {
        p.parent().map(|d| d.to_path_buf())
    }) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("ext.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "{msg}");
        }
    }
}

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const IPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IPC_IO_TIMEOUT: Duration = Duration::from_secs(5);

type EventQueue = Arc<Mutex<Vec<String>>>;

/// 全局事件队列（系统消息循环线程与托盘宿主 wndproc 共享）。
static GLOBAL_EVENTS: std::sync::OnceLock<EventQueue> = std::sync::OnceLock::new();

/// 窗口显隐状态（热键 toggle 用）。
static WINDOW_SHOWN: AtomicBool = AtomicBool::new(true);

/// 执行窗口显示/隐藏（即时），返回新状态。
fn show_or_hide(shown: bool) -> bool {
    let mode = if shown { "active" } else { "pass" };
    if let Err(e) = win_set_mode(mode) {
        ext_log(&format!("[ext] win_set_mode({mode}) 失败: {e}"));
    }
    shown
}

fn main() {
    let mut json_str = String::new();
    std::io::stdin().read_to_string(&mut json_str).unwrap_or_default();
    let cfg: Value = serde_json::from_str(&json_str).unwrap_or(json!({}));
    let port = cfg["nlPort"].as_str().unwrap_or("0").to_string();
    let token = cfg["nlToken"].as_str().unwrap_or("").to_string();
    let ext_id = cfg["nlExtensionId"].as_str().unwrap_or("").to_string();
    let conn_token = cfg["nlConnectToken"].as_str().unwrap_or("").to_string();
    eprintln!("[ext] 启动 port={port} ext={ext_id}");

    let url = format!("ws://127.0.0.1:{port}?extensionId={ext_id}&connectToken={conn_token}");
    let mut ws = match tungstenite::connect(url::Url::parse(&url).unwrap()) {
        Ok((socket, _resp)) => socket,
        Err(e) => {
            eprintln!("[ext] 连接失败: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("[ext] WebSocket 已连接");

    let events: EventQueue = Arc::new(Mutex::new(Vec::new()));
    let _ = GLOBAL_EVENTS.set(Arc::clone(&events));
    let _ = std::thread::spawn({
        let events = Arc::clone(&events);
        move || system_loop(events)
    });

    let id_counter: AtomicU64 = AtomicU64::new(0);
    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(tungstenite::Error::ConnectionClosed) => break,
            Err(e) => {
                eprintln!("[ext] 连接错误: {e}");
                break;
            }
        };
        match msg {
            tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_) => {}
            tungstenite::Message::Close(_) => break,
            tungstenite::Message::Text(payload) => {
                if let Ok(v) = serde_json::from_str::<Value>(&payload) {
                    if v["event"].as_str() == Some("runRust") {
                        handle_run_rust(&mut ws, &v["data"], &token, &id_counter, &events);
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!("[ext] 退出");
}

fn handle_run_rust(
    ws: &mut tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>,
    data: &Value,
    token: &str,
    id_counter: &AtomicU64,
    events: &EventQueue,
) {
    let function = data["function"].as_str().unwrap_or("");
    match function {
        "poll" => {
            let req_id = data["id"].as_str().unwrap_or("").to_string();
            let evs: Vec<String> = events.lock().unwrap().drain(..).collect();
            if !evs.is_empty() {
                ext_log(&format!("[ext] poll 返回事件: {:?}", evs));
            }
            let n = id_counter.fetch_add(1, Ordering::SeqCst);
            let packet = json!({
                "id": format!("id-{n}"),
                "method": "app.broadcast",
                "accessToken": token,
                "data": json!({
                    "event": "pollResult",
                    // 关键：回带请求 id（前端按 id 匹配）+ 统一 data 层
                    "data": {"id": req_id, "data": {"events": evs}},
                }),
            });
            let _ = ws.send(tungstenite::Message::Text(packet.to_string()));
        }
        "winmode" => {
            let mode = data["mode"].as_str().unwrap_or("active");
            let result = win_set_mode(mode);
            let n = id_counter.fetch_add(1, Ordering::SeqCst);
            let packet = json!({
                "id": format!("id-{n}"),
                "method": "app.broadcast",
                "accessToken": token,
                "data": json!({"event": "winmodeResult", "data": {
                    "mode": mode,
                    "ok": result.is_ok(),
                    "err": result.err().unwrap_or_default(),
                }}),
            });
            let _ = ws.send(tungstenite::Message::Text(packet.to_string()));
        }
        "ipc" => {
            let id = data["id"].as_str().unwrap_or("").to_string();
            let cmd = data["cmd"].as_str().unwrap_or("").to_string();
            let params = data["params"].clone();
            let addr = data["addr"].as_str().unwrap_or("127.0.0.1:17370").to_string();
            let ipc_token = data["token"].as_str().map(|t| t.to_string());
            let result = ipc_call(&addr, ipc_token.as_deref(), &cmd, &params);
            let (status, payload) = match result {
                Ok(v) => ("ok", v),
                Err(e) => ("error", json!(e)),
            };
            let n = id_counter.fetch_add(1, Ordering::SeqCst);
            let packet = json!({
                "id": format!("id-{n}"),
                "method": "app.broadcast",
                "accessToken": token,
                "data": json!({
                    "event": "ipcResult",
                    "data": json!({"id": id, "status": status, "data": payload}),
                }),
            });
            let _ = ws.send(tungstenite::Message::Text(packet.to_string()));
        }
        _ => {}
    }
}

fn ipc_call(addr: &str, token: Option<&str>, cmd: &str, params: &Value) -> Result<Value, String> {
    let mut request = json!({"cmd": cmd});
    let has_params = !params.is_null()
        && !(params.is_object() && params.as_object().unwrap().is_empty());
    if has_params {
        request["params"] = params.clone();
    }
    let envelope = json!({"token": token, "request": request});

    let mut stream =
        connect_with_timeout(addr).map_err(|e| format!("无法连接 anm-core 服务 {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(IPC_IO_TIMEOUT))
        .map_err(|e| e.to_string())?;

    let line = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut out = String::new();
    reader
        .read_line(&mut out)
        .map_err(|e| format!("读取响应失败: {e}"))?;
    let resp: Value = serde_json::from_str(out.trim()).map_err(|e| format!("响应解析失败: {e}"))?;
    if resp["ok"].as_bool().unwrap_or(false) {
        Ok(resp["data"].clone())
    } else {
        Err(resp["error"].as_str().unwrap_or("服务返回未知错误").to_string())
    }
}

fn connect_with_timeout(addr: &str) -> Result<TcpStream, String> {
    let mut addrs: Vec<_> = addr
        .to_socket_addrs()
        .map_err(|e| format!("地址解析失败: {e}"))?
        .collect();
    addrs.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
    let mut last: Option<std::io::Error> = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, IPC_CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .map(|e| e.to_string())
        .unwrap_or_else(|| "无可用解析结果".to_string()))
}

// ---------------------------------------------------------------------------
// Windows：系统消息循环（热键 + 托盘）+ 窗口控制
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn system_loop(_events: EventQueue) {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_SHIFT};
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW};
    use windows_sys::core::w;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DestroyMenu, GetCursorPos,
        GetMessageW, LoadIconW, PeekMessageW, RegisterClassW, SetForegroundWindow,
        TrackPopupMenu, MF_STRING, MSG, PM_NOREMOVE, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP,
        WM_HOTKEY, WM_LBUTTONDOWN, WM_RBUTTONUP, WNDCLASSW,
    };

    const TRAY_MSG: u32 = WM_APP + 1;
    const WM_CONTEXTMENU: u32 = 0x007B;

    unsafe {
        let mut msg0: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg0, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);

        if RegisterHotKey(std::ptr::null_mut(), 1, MOD_ALT | MOD_SHIFT, b'Z' as u32) == 0 {
            let e = GetLastError();
            ext_log(&format!("[ext] 热键注册失败: {e}"));
        } else {
            ext_log("[ext] 热键注册成功");
        }

        // 托盘宿主窗口
        let hinst = GetModuleHandleW(std::ptr::null());
        let class_w: Vec<u16> = "AnmExtTrayHost"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(tray_host_wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = class_w.as_ptr();
        let host = if RegisterClassW(&wc) != 0 {
            CreateWindowExW(
                0,
                class_w.as_ptr(),
                w!(""),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null_mut(),
            )
        } else {
            std::ptr::null_mut()
        };

        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = host;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = TRAY_MSG;
        nid.hIcon = LoadIconW(hinst, 1 as *const u16);
        let tip: Vec<u16> = "anm-tray".encode_utf16().collect();
        let n = tip.len().min(127);
        std::ptr::copy_nonoverlapping(tip.as_ptr(), nid.szTip.as_mut_ptr(), n);
        nid.szTip[n] = 0;
        let add_ok = Shell_NotifyIconW(NIM_ADD, &mut nid);
        ext_log(&format!("[ext] 托盘 NIM_ADD: {} (host={:?}, icon={:?})", add_ok != 0, host, nid.hIcon));

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY && msg.wParam == 1 {
                // 热键即时响应：切换窗口显隐（不经前端，无轮询延迟）
                ext_log("[ext] 热键 → 切换");
                WINDOW_SHOWN.store(show_or_hide(!WINDOW_SHOWN.load(Ordering::SeqCst)), Ordering::SeqCst);
            }
            if msg.message == TRAY_MSG {
                ext_log(&format!("[ext] 托盘回调 lParam=0x{:x}", msg.lParam as u32));
                match msg.lParam as u32 {
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let menu = CreatePopupMenu();
                        AppendMenuW(menu, MF_STRING, 101, w!("激活"));
                        AppendMenuW(menu, MF_STRING, 103, w!("退出"));
                        let mut pt: POINT = std::mem::zeroed();
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(msg.hwnd);
                        let cmd = TrackPopupMenu(
                            menu,
                            TPM_RIGHTBUTTON | TPM_RETURNCMD,
                            pt.x,
                            pt.y,
                            0,
                            msg.hwnd,
                            std::ptr::null_mut(),
                        );
                        DestroyMenu(menu);
                        match cmd as usize {
                            101 => {
                                ext_log("[ext] 托盘 → 激活");
                                WINDOW_SHOWN.store(show_or_hide(true), Ordering::SeqCst);
                            }
                            103 => std::process::exit(0),
                            _ => {}
                        }
                    }
                    WM_LBUTTONDOWN => {
                        ext_log("[ext] 托盘左键 → 激活");
                        WINDOW_SHOWN.store(show_or_hide(true), Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
            let _ = (msg.hwnd, msg.message, msg.wParam, msg.lParam);
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn tray_host_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DefWindowProcW, DestroyMenu, GetCursorPos, PostQuitMessage,
        SetForegroundWindow, TrackPopupMenu, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DESTROY,
        WM_LBUTTONDOWN, WM_RBUTTONUP,
    };
    use windows_sys::core::w;
    const WM_CONTEXTMENU: u32 = 0x007B;
    const TRAY_MSG: u32 = 0x8000 + 1; // WM_APP + 1
    if msg == TRAY_MSG {
        // 托盘回调（SendMessage 路径）：右键菜单 / 左键激活 → 事件入队
        let evt: Option<String> = unsafe {
            match lparam as u32 {
            WM_LBUTTONDOWN => Some("tray_activate".into()),
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                let menu = CreatePopupMenu();
                AppendMenuW(menu, MF_STRING, 101, w!("激活"));
                AppendMenuW(menu, MF_STRING, 103, w!("退出"));
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(
                    menu,
                    TPM_RIGHTBUTTON | TPM_RETURNCMD,
                    pt.x,
                    pt.y,
                    0,
                    hwnd,
                    std::ptr::null_mut(),
                );
                DestroyMenu(menu);
                match cmd as usize {
                    101 => Some("tray_activate".into()),
                    102 => Some("tray_hide".into()),
                    103 => Some("tray_exit".into()),
                    _ => None,
                }
            }
            _ => None,
        }
        };
        if let Some(evt) = evt {
            ext_log(&format!("[ext] 托盘 wndproc 事件: {evt}"));
            if let Some(q) = GLOBAL_EVENTS.get() {
                q.lock().unwrap().push(evt);
            }
        }
        return 0;
    }
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

#[cfg(not(windows))]
fn system_loop(_events: EventQueue) {}

#[cfg(windows)]
fn win_set_mode(mode: &str) -> Result<(), String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, SetWindowPos, ShowWindow, HWND_TOPMOST, SW_HIDE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE, SW_SHOW,
    };
    unsafe {
        let title: Vec<u16> = "anm-tray".encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd: HWND = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            return Err("未找到 anm-tray 窗口".to_string());
        }
        if mode == "pass" {
            ShowWindow(hwnd, SW_HIDE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            SetForegroundWindow(hwnd);
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn win_set_mode(_mode: &str) -> Result<(), String> {
    Ok(())
}
