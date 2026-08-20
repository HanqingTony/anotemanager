//! anm-daemon：后台守护进程。
//!
//! 职责：
//! - 常驻后台；
//! - 监听笔记目录变动（非唯一入口：只观察，不拦截任何外部修改）；
//! - 变动后重建索引并落盘（`~/.anm/index.jsonl`）；
//! - 通过 TCP（JSON 行协议）向 CLI / MCP / tray 提供查询服务。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use anm_core::{config::Config, index, index::IndexEntry};

/// 默认 TCP 端口；可用环境变量 ANM_DAEMON_PORT 覆盖
pub const DEFAULT_PORT: u16 = 17370;

#[derive(Debug, Deserialize)]
struct Request {
    cmd: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    keyword: String,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() -> Result<()> {
    let cfg = Config::load()?;
    let port = std::env::var("ANM_DAEMON_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    // 初始索引
    println!("anm-daemon: 构建初始索引 {}", cfg.root.display());
    let entries = index::build_index(&cfg.root)?;
    index::save_index(&cfg.index_path, &entries)?;
    println!("anm-daemon: 索引 {} 条笔记", entries.len());

    let shared: Arc<Mutex<Vec<IndexEntry>>> = Arc::new(Mutex::new(entries));

    // ---- 文件监听线程 ----
    let (evt_tx, evt_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = evt_tx.send(res);
        },
        NotifyConfig::default(),
    )
    .context("创建文件监听器失败")?;
    watcher
        .watch(&cfg.root, RecursiveMode::Recursive)
        .with_context(|| format!("监听 {} 失败", cfg.root.display()))?;
    println!("anm-daemon: 正在监听 {}", cfg.root.display());

    let shared_w = Arc::clone(&shared);
    let root_w = cfg.root.clone();
    let index_path_w = cfg.index_path.clone();
    thread::spawn(move || {
        loop {
            match evt_rx.recv() {
                Ok(res) => {
                    if let Err(e) = res {
                        eprintln!("anm-daemon: 监听事件错误: {e}");
                        continue;
                    }
                    // 防抖：等 500ms，并清空积压事件，避免连续写入触发反复重建
                    thread::sleep(Duration::from_millis(500));
                    while evt_rx.try_recv().is_ok() {}
                    match index::build_index(&root_w) {
                        Ok(entries) => {
                            let n = entries.len();
                            if let Err(e) = index::save_index(&index_path_w, &entries) {
                                eprintln!("anm-daemon: 保存索引失败: {e}");
                            }
                            if let Ok(mut guard) = shared_w.lock() {
                                *guard = entries;
                            }
                            println!("anm-daemon: 索引已更新（{} 条）", n);
                        }
                        Err(e) => eprintln!("anm-daemon: 重建索引失败: {e}"),
                    }
                }
                Err(_) => break,
            }
        }
    });

    // ---- TCP 服务 ----
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("绑定 127.0.0.1:{port} 失败"))?;
    println!("anm-daemon: TCP 服务监听 127.0.0.1:{port}");
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let shared_c = Arc::clone(&shared);
                thread::spawn(move || handle_conn(s, shared_c));
            }
            Err(e) => eprintln!("anm-daemon: 接受连接失败: {e}"),
        }
    }
    Ok(())
}

fn handle_conn(stream: TcpStream, shared: Arc<Mutex<Vec<IndexEntry>>>) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let resp = match serde_json::from_str::<Request>(&line) {
        Ok(req) => dispatch(&req, &shared),
        Err(e) => Response {
            ok: false,
            data: None,
            error: Some(format!("请求解析失败: {e}")),
        },
    };
    let out = match serde_json::to_string(&resp) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut writer = match reader.get_ref().try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let _ = writeln!(writer, "{}", out);
}

fn dispatch(req: &Request, shared: &Mutex<Vec<IndexEntry>>) -> Response {
    let entries = match shared.lock() {
        Ok(g) => g,
        Err(_) => {
            return Response {
                ok: false,
                data: None,
                error: Some("索引锁不可用".to_string()),
            }
        }
    };
    let data = match req.cmd.as_str() {
        "ls" => serde_json::to_value(&*entries),
        "find_tag" => {
            let hits = index::find_by_tag(&entries, &req.tags);
            serde_json::to_value(&hits)
        }
        "search" => {
            let hits = index::find_by_title(&entries, &req.keyword);
            serde_json::to_value(&hits)
        }
        "tags" => {
            let mut set: Vec<String> = Vec::new();
            for e in entries.iter() {
                for t in &e.tags {
                    if !set.contains(t) {
                        set.push(t.clone());
                    }
                }
            }
            set.sort();
            serde_json::to_value(&set)
        }
        other => {
            return Response {
                ok: false,
                data: None,
                error: Some(format!("未知命令: {other}")),
            }
        }
    };
    match data {
        Ok(v) => Response {
            ok: true,
            data: Some(v),
            error: None,
        },
        Err(e) => Response {
            ok: false,
            data: None,
            error: Some(format!("序列化失败: {e}")),
        },
    }
}
