//! 人机 HTTP API（浏览器前端通道）：POST /api/ipc
//!
//! 背景：anm 前端纯前端化后，浏览器无法直连 TCP IPC——本端点把 IPC 的
//! Envelope 信封协议原样搬到 HTTP：请求体 `{"token":..,"request":{..}}`，
//! 响应体 `{"ok":true,"data":..}` 或 `{"ok":false,"error":".."}`，
//! 与 TCP IPC 完全同构（复用 check_token / dispatch，同权限同白名单）。
//!
//! 安全：绑定 host 与 IPC 一致（默认 0.0.0.0，跨机浏览器访问）；
//! 鉴权仍由 `[server] token` 令牌把关（信封内携带，与 TCP 一致）；
//! CORS 全放行 + 令牌 = 内网宽松模式（readme §17 语义不变：
//! 令牌是主要防线，公网暴露仍需隧道）。
//!
//! 端口：默认 17373（环境变量 ANM_HTTP_API_PORT 覆盖）。

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use anm_core::protocol::Envelope;

use crate::server;

/// 人机 HTTP API 默认端口
pub const DEFAULT_HTTP_API_PORT: u16 = 17373;

const CORS_HEADERS: &str = "\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: POST, OPTIONS\r\n\
Access-Control-Allow-Headers: Content-Type\r\n\
Access-Control-Max-Age: 86400\r\n";

/// 启动人机 HTTP API 服务（常驻）。
pub async fn run_http(cfg: anm_core::config::Config) -> Result<()> {
    let port = std::env::var("ANM_HTTP_API_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(DEFAULT_HTTP_API_PORT);
    let addr = format!("{}:{port}", cfg.server.host);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("绑定 HTTP API 地址 {addr} 失败（端口被占用？）"))?;
    println!("anm-core: HTTP API 已启动 → http://{addr}/api/ipc（浏览器前端通道）");
    let cfg = Arc::new(cfg);
    loop {
        let (stream, _peer) = listener.accept().await?;
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_http(stream, &cfg).await {
                eprintln!("anm-core: HTTP API 连接处理失败: {e:#}");
            }
        });
    }
}

/// 处理一个 HTTP 连接（仅支持 POST /api/ipc 与 CORS 预检 OPTIONS）。
async fn handle_http(mut stream: TcpStream, cfg: &anm_core::config::Config) -> Result<()> {
    let mut reader = BufReader::new(&mut stream);
    // 读请求行 + 头（到空行）
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse::<usize>().ok();
        }
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    let method = parts.first().copied().unwrap_or("");
    let path = parts.get(1).copied().unwrap_or("");

    // CORS 预检
    if method == "OPTIONS" {
        let _ = stream
            .write_all(
                format!(
                    "HTTP/1.1 204 No Content\r\n{CORS_HEADERS}\r\n"
                )
                .as_bytes(),
            )
            .await;
        let _ = stream.flush().await;
        return Ok(());
    }

    if method != "POST" || path != "/api/ipc" {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nnot found")
            .await;
        return Ok(());
    }

    // 读 body
    let mut body = Vec::new();
    if let Some(len) = content_length {
        if len > 8 * 1024 * 1024 {
            let _ = stream.write_all(b"HTTP/1.1 413 Payload Too Large\r\n\r\n").await;
            return Ok(());
        }
        body.resize(len, 0);
        reader.read_exact(&mut body).await?;
    }

    let resp_json = match serde_json::from_slice::<Envelope>(&body) {
        Ok(env) => {
            let resp = match server::check_token(cfg, env.token.as_deref()) {
                Ok(()) => server::dispatch(cfg, env.request),
                Err(msg) => anm_core::protocol::Response::err(msg),
            };
            serde_json::to_string(&resp).unwrap_or_else(|_| r#"{"ok":false,"error":"序列化失败"}"#.into())
        }
        Err(e) => format!(
            r#"{{"ok":false,"error":"请求解析失败: {}"}}"#,
            e.to_string().replace('"', "'")
        ),
    };

    let _ = stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp_json.len(),
                resp_json
            )
            .as_bytes(),
        )
        .await;
    let _ = stream.flush().await;
    Ok(())
}
