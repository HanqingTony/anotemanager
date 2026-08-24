// anm-tauri 后端：窗口 / 托盘 / 全局热键 / IPC 转发（TCP → anm-core）/ 配置持久化。
//
// 架构与 win32 版（v2.5 已归档）一致：前端（renderer/index.html）负责全部 UI
// 与业务，Rust 侧只做系统集成：
//   - 透明全屏置顶窗口（tauri.conf.json，原生 fullscreen 覆盖任务栏）
//   - 全局热键（默认 Alt+Shift+Z，可经「设置快捷键…」重设并持久化）
//   - 系统托盘（显示 / 设置服务地址… / 设置快捷键… / 退出）
//   - TCP 转发：前端 invoke → Rust 连 anm-core（协议信封与 anm_core::protocol 一致）
//   - 配置持久化：%APPDATA%/anm-tauri/config.json（服务地址 / 令牌 / 热键）
//   - 系统打开（ShellExecuteExW + SEE_MASK_FLAG_NO_UI；失败由前端 toast）
//
// 关键差异 vs Electron：Tauri 的 fullscreen 是原生全屏窗口（WS_POPUP 铺满
// 显示器物理区域），会真正盖住任务栏，无需 PowerShell 隐藏任务栏。

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager, State};

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 持久化配置（%APPDATA%/anm-tauri/config.json）。
#[derive(Serialize, Deserialize, Default, Clone)]
struct TrayConfig {
    server_addr: Option<String>,
    server_token: Option<String>,
    hotkey: Option<String>,
}

fn config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA")
        .map(|p| std::path::PathBuf::from(p).join("anm-tauri").join("config.json"))
}

fn load_config() -> TrayConfig {
    let Some(path) = config_path() else {
        return TrayConfig::default();
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &TrayConfig) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, text);
    }
}

struct AppState {
    addr: Mutex<String>,
    token: Mutex<Option<String>>,
    /// 当前已注册的全局快捷键（字符串形式，与 config 一致）
    hotkey: Mutex<Option<String>>,
    /// 当前注册的 Shortcut（重设时先 unregister）
    hotkey_shortcut: Mutex<Option<tauri_plugin_global_shortcut::Shortcut>>,
}

const DEFAULT_HOTKEY: &str = "Alt+Shift+Z";

impl Default for AppState {
    fn default() -> Self {
        let cfg = load_config();
        Self {
            addr: Mutex::new(
                cfg.server_addr
                    .clone()
                    .or_else(|| std::env::var("ANM_SERVER_ADDR").ok())
                    .unwrap_or_else(|| "127.0.0.1:17370".into()),
            ),
            token: Mutex::new(
                cfg.server_token
                    .clone()
                    .or_else(|| std::env::var("ANM_SERVER_TOKEN").ok()),
            ),
            hotkey: Mutex::new(Some(cfg.hotkey.unwrap_or_else(|| DEFAULT_HOTKEY.to_string()))),
            hotkey_shortcut: Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------------------
// IPC 转发
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct IpcResp {
    status: &'static str,
    data: serde_json::Value,
}

fn ok(data: serde_json::Value) -> IpcResp {
    IpcResp { status: "ok", data }
}
fn err(msg: String) -> IpcResp {
    IpcResp {
        status: "error",
        data: serde_json::Value::String(msg),
    }
}

// TCP 转发：信封 {"token":.., "request":{"cmd":..,"params":..}}；
// unit 命令不带 params 字段（与 anm_core::protocol 序列化一致）。
fn tcp_call(addr: &str, token: Option<&str>, cmd: &str, params: Option<&serde_json::Value>) -> IpcResp {
    let mut request = serde_json::Map::new();
    request.insert("cmd".into(), serde_json::Value::String(cmd.to_string()));
    let has_params = match params {
        Some(p) => !(p.is_object() && p.as_object().map(|o| o.is_empty()).unwrap_or(false)),
        None => false,
    };
    if has_params {
        request.insert("params".into(), params.unwrap().clone());
    }
    let envelope = serde_json::json!({ "token": token, "request": serde_json::Value::Object(request) });

    let sock = match TcpStream::connect(addr) {
        Ok(s) => s,
        Err(e) => return err(format!("无法连接 anm-core {addr}: {e}")),
    };
    let _ = sock.set_read_timeout(Some(Duration::from_secs(8)));
    let _ = sock.set_write_timeout(Some(Duration::from_secs(8)));
    let mut sock = sock;
    if let Err(e) = sock.write_all((envelope.to_string() + "\n").as_bytes()) {
        return err(format!("发送失败: {e}"));
    }
    let mut line = String::new();
    match BufReader::new(sock).read_line(&mut line) {
        Ok(0) => err("服务无响应".into()),
        Ok(_) => match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(resp) => {
                if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                    ok(resp.get("data").cloned().unwrap_or(serde_json::Value::Null))
                } else {
                    err(resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("服务错误")
                        .to_string())
                }
            }
            Err(e) => err(format!("响应解析失败: {e}")),
        },
        Err(e) => err(format!("读取响应失败: {e}")),
    }
}

#[tauri::command]
fn anm_ipc(state: State<AppState>, cmd: String, params: Option<serde_json::Value>) -> IpcResp {
    let addr = state.addr.lock().unwrap().clone();
    let token = state.token.lock().unwrap().clone();
    tcp_call(&addr, token.as_deref(), &cmd, params.as_ref())
}

/// 设置服务地址 / 令牌并持久化（前端设置对话框用）。
#[tauri::command]
fn anm_set_config(state: State<AppState>, cfg: Option<serde_json::Value>) -> IpcResp {
    let mut save = load_config();
    if let Some(cfg) = cfg {
        if let Some(addr) = cfg
            .get("addr")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            *state.addr.lock().unwrap() = addr.trim().to_string();
            save.server_addr = Some(addr.trim().to_string());
        }
        if let Some(token) = cfg.get("token") {
            let t = token
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            *state.token.lock().unwrap() = t.clone();
            save.server_token = t;
        }
    }
    save_config(&save);
    ok(serde_json::json!({ "ok": true, "addr": state.addr.lock().unwrap().clone() }))
}

/// 读取当前配置（前端显示用）。
#[tauri::command]
fn anm_get_config(state: State<AppState>) -> IpcResp {
    ok(serde_json::json!({
        "addr": state.addr.lock().unwrap().clone(),
        "token": state.token.lock().unwrap().clone().unwrap_or_default(),
        "hotkey": state.hotkey.lock().unwrap().clone().unwrap_or_else(|| DEFAULT_HOTKEY.to_string()),
    }))
}

/// 重设全局快捷键（前端「设置快捷键…」用）：先取消旧的，注册新的，持久化。
#[tauri::command]
fn anm_set_hotkey(app: tauri::AppHandle, state: State<AppState>, keys: String) -> IpcResp {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let keys = keys.trim().to_string();
    if keys.is_empty() {
        return err("快捷键不能为空".into());
    }
    // 解析校验
    let sc = match tauri_plugin_global_shortcut::Shortcut::from_str(&keys) {
        Ok(s) => s,
        Err(e) => return err(format!("无法解析快捷键 {keys}: {e}")),
    };
    // 取消旧的
    if let Some(old) = state.hotkey_shortcut.lock().unwrap().take() {
        let _ = app.global_shortcut().unregister(old);
    }
    // 注册新的
    let hk = keys.clone();
    let reg = app.global_shortcut().on_shortcut(sc.clone(), |app, _sc, event| {
        use tauri_plugin_global_shortcut::ShortcutState;
        if event.state == ShortcutState::Pressed {
            if let Some(win) = app.get_webview_window("main") {
                if win.is_visible().unwrap_or(false) {
                    hide_main(&win);
                } else {
                    show_main(&win);
                }
            }
        }
    });
    match reg {
        Ok(()) => {
            *state.hotkey.lock().unwrap() = Some(hk.clone());
            *state.hotkey_shortcut.lock().unwrap() = Some(sc);
            let mut cfg = load_config();
            cfg.hotkey = Some(hk);
            save_config(&cfg);
            ok(serde_json::json!({ "ok": true, "hotkey": keys }))
        }
        Err(e) => err(format!("注册快捷键失败: {e}")),
    }
}

/// 隐藏覆盖层（前端点击空白取消激活用）。
#[tauri::command]
fn anm_hide(app: tauri::AppHandle) -> IpcResp {
    if let Some(win) = app.get_webview_window("main") {
        hide_main(&win);
    }
    ok(serde_json::json!({ "ok": true }))
}

/// 系统默认方式打开路径（ShellExecuteExW + SEE_MASK_FLAG_NO_UI：
/// 打不开时不弹系统错误框，由前端 toast）。
#[tauri::command]
fn anm_open_path(path: String) -> IpcResp {
    if open_with_default_handler(&path) {
        ok(serde_json::json!({ "ok": true }))
    } else {
        ok(serde_json::json!({
            "ok": false,
            "msg": format!("无法打开：{path}（此路径只在服务端机器上，本机没有对应文件）")
        }))
    }
}

#[cfg(windows)]
fn open_with_default_handler(path: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    // WSL 路径 → Windows 路径：/mnt/<盘>/… → <盘>:\…；其余原样
    // （服务端路径在本机打不开时由调用方提示，不弹系统错误框）。
    let lower = path.to_ascii_lowercase();
    let win_path = if let Some(rest) = lower.strip_prefix("/mnt/") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b'/' {
            let drive = bytes[0] as char;
            let tail = &path["/mnt/".len() + 2..];
            format!("{}:\\{}", drive.to_ascii_uppercase(), tail.replace('/', "\\"))
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    let wide: Vec<u16> = win_path.encode_utf16().chain(std::iter::once(0)).collect();
    let verb: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = wide.as_ptr();
    sei.nShow = SW_SHOWNORMAL as i32;
    unsafe { ShellExecuteExW(&mut sei) != 0 }
}

#[cfg(not(windows))]
fn open_with_default_handler(_path: &str) -> bool {
    false
}

// ---------------------------------------------------------------------------
// 窗口显隐
// ---------------------------------------------------------------------------

fn show_main(win: &tauri::WebviewWindow) {
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("anm-event", "shown"); // 前端刷新卡片
}
fn hide_main(win: &tauri::WebviewWindow) {
    let _ = win.hide();
    let _ = win.emit("anm-event", "hidden");
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

pub fn run() {
    let single_instance = tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
        }
    });

    tauri::Builder::default()
        .plugin(single_instance)
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            anm_ipc,
            anm_set_config,
            anm_get_config,
            anm_set_hotkey,
            anm_hide,
            anm_open_path
        ])
        .setup(|app| {
            // 全局热键（配置的或默认）
            use tauri_plugin_global_shortcut::GlobalShortcutExt;
            let hk = app
                .state::<AppState>()
                .hotkey
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
            let sc = tauri_plugin_global_shortcut::Shortcut::from_str(&hk).unwrap_or_else(|_| {
                tauri_plugin_global_shortcut::Shortcut::from_str(DEFAULT_HOTKEY).unwrap()
            });
            app.global_shortcut()
                .on_shortcut(sc.clone(), |app, _sc, event| {
                    use tauri_plugin_global_shortcut::ShortcutState;
                    if event.state == ShortcutState::Pressed {
                        if let Some(win) = app.get_webview_window("main") {
                            if win.is_visible().unwrap_or(false) {
                                hide_main(&win);
                            } else {
                                show_main(&win);
                            }
                        }
                    }
                })
                .expect("注册热键失败");
            *app.state::<AppState>().hotkey_shortcut.lock().unwrap() = Some(sc);

            // 系统托盘：显示 / 设置服务地址… / 设置快捷键… / 退出
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
            let show_i = MenuItem::with_id(app, "show", "显示", true, None::<&str>)?;
            let settings_i = MenuItem::with_id(app, "settings", "设置服务地址…", true, None::<&str>)?;
            let hotkey_i = MenuItem::with_id(app, "hotkey", "设置快捷键…", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &settings_i, &hotkey_i, &quit_i])?;
            // 图标：include_bytes 直接打包（交叉编译时 exe 资源嵌入不可靠，
            // default_window_icon() 可能为 None）
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/anm.ico"))
                .expect("图标解析失败");
            let _tray = TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .tooltip("anm-tray")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            show_main(&win);
                        }
                    }
                    // 设置对话框由前端渲染（与覆盖层风格统一）；
                    // 托盘场景下窗口通常是隐藏的——必须先显示窗口，
                    // 否则对话框 DOM 显示在看不见的窗口里（"点了没反应"）
                    "settings" => {
                        if let Some(win) = app.get_webview_window("main") {
                            show_main(&win);
                        }
                        let _ = app.emit("anm-menu", "settings");
                    }
                    "hotkey" => {
                        if let Some(win) = app.get_webview_window("main") {
                            show_main(&win);
                        }
                        let _ = app.emit("anm-menu", "hotkey");
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            if win.is_visible().unwrap_or(false) {
                                hide_main(&win);
                            } else {
                                show_main(&win);
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("tauri 运行失败");
}
