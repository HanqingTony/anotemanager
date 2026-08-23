//! IPC 客户端（跨平台）：连接 anm-core 服务的 `[server]` 端点。
//!
//! 与 anm-cli / anm-win-tray 的客户端同构（协议类型共用 anm_core::protocol）。
//! 服务地址由环境变量决定：
//! - 默认 `127.0.0.1:17370`（Linux/Windows 本机服务，或 WSL2 localhost 转发）；
//! - 环境变量 `ANM_SERVER_ADDR` 覆盖（如 `192.168.0.101:17370`，跨机测试）。

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use anm_core::protocol::{Envelope, Request, Response};

/// 默认服务地址（与 anm-core `[server]` 默认一致）。
const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:17370";

/// 连接超时：服务不可达（防火墙丢包 / 地址黑洞）时快速失败，
/// 避免调用方（UI 线程）长时间冻结。局域网正常连接 <10ms。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// 读写超时：防止服务挂起时客户端无限等待。
const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// 最近一次 IPC 调用是否成功（外壳右上角「连接状态」显示用）。
static LAST_OK: AtomicBool = AtomicBool::new(false);

/// 查询最近一次 IPC 调用的成败。
pub fn last_ok() -> bool {
    LAST_OK.load(Ordering::Relaxed)
}

/// 运行时服务地址覆盖（托盘「设置服务地址」菜单写入；优先级高于环境变量）。
static SERVER_ADDR_OVERRIDE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 运行时访问令牌覆盖（托盘「设置服务地址」对话框一并写入；优先级高于环境变量）。
static SERVER_TOKEN_OVERRIDE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 设置服务地址覆盖（`None` = 清除覆盖，回落环境变量/默认值）。
///
/// 平台外壳在启动时把持久化配置注入这里，设置对话框修改后再次调用。
pub fn set_server_addr_override(addr: Option<String>) {
    if let Ok(mut guard) = SERVER_ADDR_OVERRIDE.lock() {
        *guard = addr;
    }
}

/// 设置访问令牌覆盖（与地址覆盖同时持久化；`None` = 清除，回落环境变量）。
pub fn set_server_token_override(token: Option<String>) {
    if let Ok(mut guard) = SERVER_TOKEN_OVERRIDE.lock() {
        *guard = token;
    }
}

/// 返回当前访问令牌：运行时覆盖 > 环境变量 `ANM_SERVER_TOKEN` > 无。
pub fn server_token() -> Option<String> {
    if let Ok(guard) = SERVER_TOKEN_OVERRIDE.lock() {
        if let Some(t) = guard.as_ref() {
            if !t.trim().is_empty() {
                return Some(t.clone());
            }
        }
    }
    std::env::var("ANM_SERVER_TOKEN").ok().filter(|t| !t.trim().is_empty())
}

/// 返回当前服务地址：运行时覆盖 > 环境变量 `ANM_SERVER_ADDR` > 默认值。
pub fn server_addr() -> String {
    if let Ok(guard) = SERVER_ADDR_OVERRIDE.lock() {
        if let Some(addr) = guard.as_ref() {
            if !addr.trim().is_empty() {
                return addr.clone();
            }
        }
    }
    std::env::var("ANM_SERVER_ADDR").unwrap_or_else(|_| DEFAULT_SERVER_ADDR.to_string())
}

/// 带超时的 TCP 连接：依次尝试解析出的每个地址，单个地址超时
/// [`CONNECT_TIMEOUT`] 即放弃换下一个。
///
/// **IPv4 优先**：Windows 上 `localhost` 会解析出 `::1` 与 `127.0.0.1`，
/// 而 WSL2 的 localhost 转发只监听 IPv4——先试 `::1` 会白等一个超时
/// 周期（每次操作多卡 2 秒）。IPv4 优先后本机/WSL 场景秒连。
fn connect_with_timeout(addr: &str) -> Result<TcpStream> {
    let mut addrs: Vec<_> = addr
        .to_socket_addrs()
        .with_context(|| format!("服务地址解析失败: {addr}"))?
        .collect();
    addrs.sort_by_key(|a| if a.is_ipv4() { 0 } else { 1 });
    let mut last: Option<std::io::Error> = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .map(Into::into)
        .unwrap_or_else(|| anyhow!("服务地址无可用解析结果: {addr}")))
}

/// 校验"主机:端口"格式（设置对话框用）。
pub fn validate_addr(addr: &str) -> Result<(), String> {
    let addr = addr.trim();
    if addr.is_empty() {
        return Err("地址不能为空".to_string());
    }
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| "格式应为 主机:端口（如 192.168.0.102:17370）".to_string())?;
    let host = host.trim();
    if host.is_empty() {
        return Err("缺少主机名".to_string());
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ':')
    {
        return Err("主机名含非法字符（仅允许字母/数字/./-/_:）".to_string());
    }
    let port: u16 = port
        .trim()
        .parse()
        .map_err(|_| format!("端口非法: {port}"))?;
    if port == 0 {
        return Err("端口不能为 0".to_string());
    }
    Ok(())
}

/// 向 anm-core 服务发送一个 IPC 请求，返回响应中的数据（JSON 值）。
///
/// - 连接失败时给出"服务未启动？先运行 `anm-core`"的操作提示；
/// - 服务返回 `ok: false` 时把错误描述原样抛出。
pub fn call(req: &Request) -> Result<serde_json::Value> {
    let result = call_inner(req);
    LAST_OK.store(result.is_ok(), Ordering::Relaxed);
    result
}

/// call 的实际实现（外层负责记录成败）。
fn call_inner(req: &Request) -> Result<serde_json::Value> {
    let addr = server_addr();
    let mut stream = connect_with_timeout(&addr).with_context(|| {
        format!("无法连接 anm-core 服务 {addr}（服务未启动？先运行 `anm-core`）")
    })?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    // 请求一行（信封：携带令牌；服务端未配置令牌时自动忽略）
    let line = serde_json::to_string(&Envelope {
        token: server_token(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 地址校验：合法/非法格式。
    #[test]
    fn addr_validation() {
        assert!(validate_addr("192.168.0.102:17370").is_ok());
        assert!(validate_addr("127.0.0.1:1").is_ok());
        assert!(validate_addr("localhost:17370").is_ok());
        assert!(validate_addr("").is_err());
        assert!(validate_addr("no-port").is_err());
        assert!(validate_addr("host:99999").is_err());
        assert!(validate_addr(":17370").is_err());
        assert!(validate_addr("host:0").is_err());
        // 手动编辑混入乱码应被拒绝
        assert!(validate_addr("311阿道夫1k3nivadf192.168.0.102:17370").is_err());
        assert!(validate_addr("192.168.0.102:17370").is_ok());
    }

    /// 覆盖优先于环境变量/默认值（设置覆盖后地址随之变化）。
    #[test]
    fn override_takes_priority() {
        set_server_addr_override(Some("192.168.0.102:17370".to_string()));
        assert_eq!(server_addr(), "192.168.0.102:17370");
        set_server_addr_override(None);
        // 清空后回落默认（测试环境无 ANM_SERVER_ADDR）
        assert_eq!(server_addr(), DEFAULT_SERVER_ADDR);
    }
}
