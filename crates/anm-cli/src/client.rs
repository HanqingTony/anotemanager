//! IPC 客户端：连接 anm-core 服务的 `[server]` 端点，发送一个请求并读取响应。
//!
//! 供两个应用（`anm`、`anw`）复用：连接 → 写一行 JSON 请求 → 读一行 JSON
//! 响应 → 返回数据或错误。服务未启动 / 连接失败时返回带操作提示的错误。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use anm_core::config::Config;
use anm_core::protocol::{Envelope, Request, Response};

/// 读写超时：防止服务挂起时客户端无限等待。
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// 向 anm-core 服务发送一个 IPC 请求，返回响应中的数据（JSON 值）。
///
/// - 地址取自配置 `[server]` 段（默认 127.0.0.1:17370）；
/// - 连接失败时给出"服务未启动？先运行 `anm-core`"的操作提示；
/// - 服务返回 `ok: false` 时把错误描述原样抛出。
pub fn call(cfg: &Config, req: &Request) -> Result<serde_json::Value> {
    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let mut stream = TcpStream::connect(&addr).with_context(|| {
        format!(
            "无法连接 anm-core 服务 {addr}（服务未启动？先运行 `anm-core`）"
        )
    })?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    // 请求一行（信封：携带配置的令牌，未配置则不带）
    let line = serde_json::to_string(&Envelope {
        token: cfg.server.token.clone(),
        request: req.clone(),
    })?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    // 响应一行
    let mut reader = BufReader::new(stream);
    let mut out = String::new();
    reader.read_line(&mut out)?;
    let resp: Response = serde_json::from_str(out.trim())?;
    if resp.ok {
        resp.data.ok_or_else(|| anyhow!("服务返回了空的成功响应"))
    } else {
        Err(anyhow!(
            "{}",
            resp.error.unwrap_or_else(|| "服务返回未知错误".to_string())
        ))
    }
}
