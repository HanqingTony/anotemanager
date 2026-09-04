//! anm-core：常驻服务（一核心三应用中的"核心"）。
//!
//! 一个进程同时提供三件事（职责与 readme §9/§10/§13 一致）：
//! 1. **文件监听**（`watch` 模块）：观察笔记目录变动，只观察、不拦截；
//! 2. **IPC 端点**（`server` 模块）：供三个应用（anm / anw / anm-win-tray）
//!    做查询 / 低风险写入，全部走现场扫描的确定性原语；
//! 3. **内置 MCP**（`mcp` 模块）：对 AI agent 暴露记忆总线——
//!    HTTP 端点随服务常驻；`--stdio` 时只跑一个 MCP stdio 会话。
//!
//! 用法：
//! - `anm-core`：启动完整服务（监听 + IPC + MCP HTTP）；
//! - `anm-core --stdio`：只跑一个 MCP stdio 会话（被 MCP 客户端 spawn）；
//! - `anm-core --http [--host H] [--port P]`：覆盖 MCP HTTP 地址后启动完整服务。

mod http_api;
mod mcp;
mod server;
mod watch;

use std::thread;

use anyhow::{anyhow, Result};

use anm_core::config::Config;

/// 程序入口：解析启动参数，分发到 MCP stdio 会话或完整服务。
///
/// - `--stdio` → 只跑一个 MCP stdio 会话（无需配置，供客户端 spawn）；
/// - 其余（含无参数）→ 启动完整服务，MCP HTTP 端点随服务常驻。
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 未 anm init 时配置读取失败：stdio 会话可容忍（工具调用会提示），
    // 完整服务必须有配置（需要笔记库根目录 / IPC / MCP 端点）。
    let cfg = Config::load().ok();

    match mcp::resolve_mode(&args, cfg.as_ref())? {
        mcp::Mode::Stdio => mcp::run_stdio().await,
        mcp::Mode::Http { host, port } => {
            let cfg = cfg.ok_or_else(|| {
                anyhow!("未找到配置，请先运行 `anm init <笔记库根目录>` 注册笔记系统")
            })?;
            run_service(cfg, host, port).await
        }
    }
}

/// 启动完整服务：文件监听线程 + IPC 服务 + MCP HTTP，三者并行常驻。
///
/// - 文件监听跑在独立系统线程（notify 自带后台线程，事件经通道回传）；
/// - IPC 与 MCP HTTP 各占一个 tokio 任务；MCP HTTP 自带 Ctrl-C 优雅退出；
/// - 任一任务出错即返回错误并结束进程（简单可靠，不做自动拉起/守护）。
async fn run_service(cfg: Config, mcp_host: String, mcp_port: u16) -> Result<()> {
    println!(
        "anm-core: 服务启动 · 笔记库 {} · IPC {}:{} · MCP http://{mcp_host}:{mcp_port}/mcp",
        cfg.root.display(),
        cfg.server.host,
        cfg.server.port
    );

    // 1. 文件监听（独立线程，阻塞直到进程退出）
    let root = cfg.root.clone();
    let watch_handle = thread::spawn(move || {
        if let Err(e) = watch::run(&root) {
            eprintln!("anm-core: 文件监听退出: {e:#}");
        }
    });

    // 2. IPC 服务 + 3. MCP HTTP（tokio 任务并行常驻）
    let ipc_cfg = cfg.clone();
    let ipc_handle = tokio::spawn(async move {
        if let Err(e) = server::run(ipc_cfg).await {
            eprintln!("anm-core: IPC 服务退出: {e:#}");
        }
    });
    let http_handle = tokio::spawn(async move {
        if let Err(e) = mcp::run_http(&mcp_host, mcp_port).await {
            eprintln!("anm-core: MCP HTTP 退出: {e:#}");
        }
    });
    // 4. 人机 HTTP API（浏览器前端通道，独立 tokio 任务）
    let api_cfg = cfg.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = http_api::run_http(api_cfg).await {
            eprintln!("anm-core: HTTP API 退出: {e:#}");
        }
    });

    // 任一个常驻任务结束（异常或 Ctrl-C）即结束进程；监听线程随进程退出
    tokio::select! {
        _ = ipc_handle => {}
        _ = http_handle => {}
        _ = api_handle => {}
    }
    drop(watch_handle);
    Ok(())
}
