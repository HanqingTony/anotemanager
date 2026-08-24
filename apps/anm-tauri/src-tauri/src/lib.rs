// anm-tauri 后端：窗口 / 托盘 / 全局热键 / IPC 转发（TCP → anm-core）。
//
// 架构与 Electron 版一致：前端（renderer/index.html）负责全部 UI 与业务，
// Rust 侧只做系统集成：
//   - 透明全屏置顶窗口（tauri.conf.json，native fullscreen 覆盖任务栏）
//   - 全局热键 Alt+Shift+Z（显示/隐藏切换）
//   - 系统托盘（激活 / 退出）
//   - TCP 转发：前端 invoke → Rust 连 anm-core（协议信封与 anm_core::protocol 一致）
//
// 关键差异 vs Electron：Tauri 的 fullscreen 是原生全屏窗口（WS_POPUP 铺满
// 显示器物理区域），会真正盖住任务栏，无需 PowerShell 隐藏任务栏。

use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Emitter, Manager, State};

struct AppState {
    addr: Mutex<String>,
    token: Mutex<Option<String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            addr: Mutex::new(
                std::env::var("ANM_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:17370".into()),
            ),
            token: Mutex::new(std::env::var("ANM_SERVER_TOKEN").ok()),
        }
    }
}

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

#[tauri::command]
fn anm_set_config(state: State<AppState>, cfg: Option<serde_json::Value>) -> IpcResp {
    if let Some(cfg) = cfg {
        if let Some(addr) = cfg
            .get("addr")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            *state.addr.lock().unwrap() = addr.trim().to_string();
        }
        if let Some(token) = cfg.get("token") {
            let t = token
                .as_str()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            *state.token.lock().unwrap() = t;
        }
    }
    ok(serde_json::json!({ "ok": true, "addr": state.addr.lock().unwrap().clone() }))
}

fn show_main(win: &tauri::WebviewWindow) {
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("anm-event", "shown"); // 前端刷新卡片
}
fn hide_main(win: &tauri::WebviewWindow) {
    let _ = win.hide();
    let _ = win.emit("anm-event", "hidden");
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![anm_ipc, anm_set_config])
        .setup(|app| {
            // 全局热键 Alt+Shift+Z（与 win32/Electron 版一致）
            use tauri_plugin_global_shortcut::{
                Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
            };
            let sc = Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::KeyZ);
            app.global_shortcut()
                .on_shortcut(sc, |app, _sc, event| {
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

            // 系统托盘：激活 / 退出
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
            let show_i = MenuItem::with_id(app, "show", "激活", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
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
