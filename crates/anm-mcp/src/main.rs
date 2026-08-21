//! anm-mcp：MCP server（官方 rmcp 实现），把 anm_core 的能力暴露给 AI agent。
//!
//! 对 agent 而言这是访问笔记系统的记忆总线（取指-加载通道），而非单纯的检索接口。
//!
//! ## 传输（默认本地 HTTP，可在 `~/.anm/config.toml` 的 `[mcp]` 段配置）
//! - `anm-mcp`：按配置启动，默认 **Streamable HTTP**（`127.0.0.1:17371`，端点 `/mcp`）；
//!   绑定的 host/port 与传输方式来自配置 `[mcp]` 段，CLI 标志（`--stdio` / `--http` /
//!   `--host` / `--port`）可覆盖。
//! - **stdio**：`anm-mcp --stdio`（供 Claude Desktop / Cursor / opencode 等 spawn，零网络依赖）
//!
//! ## 安全边界（与设计文档一致）
//! - 路径白名单：所有 path / dir 参数经 `anm_core::path` 校验，仅允许笔记系统根目录内；
//! - 只读优先：写操作仅 `new` / `write_inbox` / `tag_sync` / `tag_add`，不暴露 shell；
//! - `read_note` 限长截断，避免 agent 上下文被整库灌爆。

use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{
    ServiceExt, ServerHandler, model::{CallToolResult, ContentBlock}, schemars, tool, tool_handler,
    tool_router,
    handler::server::wrapper::Parameters,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Serialize;
use serde_json::json;

use anm_core::{
    config::{Config, McpTransport}, inbox, notes, path, query, tags, tree,
};

/// `read_note` 默认截断长度（字符数）
const DEFAULT_READ_LIMIT: usize = 8000;

// ---------------------------------------------------------------------------
// 参数结构（每个工具一个；rmcp 由它们生成 JSON Schema 并做参数校验）
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NoParams {}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct FindTagParams {
    /// 标签名数组（不含 @ 前缀），任一命中
    tags: Vec<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct KeywordParams {
    /// 标题 / 文件名关键字（子串匹配，大小写不敏感）
    keyword: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ContentSearchParams {
    /// 搜索关键词
    keyword: String,
    /// 返回条数上限（默认 20）
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ReadNoteParams {
    /// 相对笔记库根的路径或绝对路径
    path: String,
    /// 字符数上限（默认 8000）
    limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListDirParams {
    /// 目录（相对笔记库根）；缺省为根目录
    dir: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RecentParams {
    /// 条数（默认 10）
    n: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NewNoteParams {
    /// 相对根目录的已存在目录
    dir: String,
    /// 标题（用作文件名）
    title: String,
    /// 可选正文；缺省生成标题行
    content: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct InboxParams {
    /// 写入 skatch.md 的内容
    text: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TagPathParams {
    /// 相对笔记库根的路径或绝对路径
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct TagAddParams {
    /// 相对笔记库根的路径或绝对路径
    path: String,
    /// 标签名数组（不含 @ 前缀）
    tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// 工具返回类型与辅助函数
// ---------------------------------------------------------------------------

/// 工具统一返回：Ok = 结果的 JSON 字符串；Err = MCP 工具级错误（isError）
type ToolOut = Result<String, CallToolResult>;

fn tool_err(msg: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("错误: {}", msg.into()))])
}

fn ok_json<T: Serialize>(v: &T) -> ToolOut {
    serde_json::to_string_pretty(v).map_err(|e| tool_err(format!("序列化结果失败: {e}")))
}

/// 加载配置；未初始化时给出可操作的提示
fn load_cfg() -> Result<Config, CallToolResult> {
    Config::load().map_err(|e| {
        tool_err(format!("{e:#}\n提示：先在笔记系统侧运行 `anm init <根目录>`"))
    })
}

/// 解析并校验 path 参数：白名单（根目录内）+ 必须是笔记文件
fn resolve_note_arg(cfg: &Config, user_path: &str) -> Result<std::path::PathBuf, CallToolResult> {
    if user_path.is_empty() {
        return Err(tool_err("缺少 path 参数"));
    }
    let resolved = path::resolve_file_in_root(&cfg.root, user_path)
        .map_err(|e| tool_err(e.to_string()))?;
    if !query::is_note_path(&resolved) {
        return Err(tool_err(format!(
            "不是笔记文件（仅支持 .md/.markdown/.txt）: {user_path}"
        )));
    }
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

/// anm MCP 服务器：薄壳，全部逻辑在 anm_core。
#[derive(Debug, Clone, Default)]
pub struct AnmServer;

#[tool_router]
impl AnmServer {
    #[tool(description = "列出笔记系统的一级目录（浏览入口）")]
    fn ls_dirs(&self, Parameters(NoParams {}): Parameters<NoParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let dirs = tree::list_top_dirs(&cfg.root).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&dirs)
    }

    #[tool(description = "列出系统中所有标签")]
    fn list_tags(&self, Parameters(NoParams {}): Parameters<NoParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let tags = query::all_tags(&cfg.root).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&tags)
    }

    #[tool(description = "按标签查找笔记。tags 为标签名数组（不含 @ 前缀），任一命中")]
    fn find_tag(&self, Parameters(FindTagParams { tags }): Parameters<FindTagParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let notes = query::find_by_tag(&cfg.root, &tags).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "按标题 / 文件名关键字查找笔记（子串匹配，大小写不敏感）")]
    fn search(&self, Parameters(KeywordParams { keyword }): Parameters<KeywordParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let notes =
            query::find_by_title(&cfg.root, &keyword).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "全文搜索笔记正文。返回命中片段（snippet）与命中次数（score），按 score 降序，limit 限制条数")]
    fn search_content(
        &self,
        Parameters(ContentSearchParams { keyword, limit }): Parameters<ContentSearchParams>,
    ) -> ToolOut {
        let cfg = load_cfg()?;
        let hits = query::search_content(&cfg.root, &keyword, limit.unwrap_or(20))
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&hits)
    }

    #[tool(description = "读取一篇笔记的完整内容（限长截断，防止上下文爆炸）。path 可为相对笔记库根的路径或绝对路径；limit 为字符数上限")]
    fn read_note(
        &self,
        Parameters(ReadNoteParams { path, limit }): Parameters<ReadNoteParams>,
    ) -> ToolOut {
        let cfg = load_cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let limit = limit.unwrap_or(DEFAULT_READ_LIMIT);
        let content = std::fs::read_to_string(&resolved)
            .map_err(|e| tool_err(format!("读取 {} 失败: {e}", resolved.display())))?;
        let total = content.chars().count();
        let shown: String = content.chars().take(limit).collect();
        ok_json(&json!({
            "path": resolved,
            "content": shown,
            "truncated": total > limit,
            "total_chars": total
        }))
    }

    #[tool(description = "列出某目录下直接包含的笔记文件（非递归）。dir 缺省为笔记库根目录")]
    fn list(&self, Parameters(ListDirParams { dir }): Parameters<ListDirParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let notes = query::list_in_dir(&cfg.root, dir.as_deref().unwrap_or("."))
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "最近修改的笔记（按最后修改时间倒序），n 为条数（默认 10）")]
    fn recent(&self, Parameters(RecentParams { n }): Parameters<RecentParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let notes =
            query::recent(&cfg.root, n.unwrap_or(10)).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "在笔记系统内新建一篇笔记：dir 为相对根目录的已存在目录，title 用作文件名，content 可选")]
    fn new(&self, Parameters(NewNoteParams { dir, title, content }): Parameters<NewNoteParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let content = content.as_deref().unwrap_or("");
        let created = notes::create_note(&cfg.root, &dir, &title, content)
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": created, "created": true }))
    }

    #[tool(description = "向默认 skatch.md（inbox 入闸缓冲）写入内容，适合记录临时想法、待办、冲动")]
    fn write_inbox(&self, Parameters(InboxParams { text }): Parameters<InboxParams>) -> ToolOut {
        let cfg = load_cfg()?;
        inbox::append(&cfg.skatch, &text).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "written": true, "skatch": cfg.skatch }))
    }

    #[tool(description = "同步一篇笔记的头部标签区：把文档中的标签行统一维护到文件头部")]
    fn tag_sync(&self, Parameters(TagPathParams { path }): Parameters<TagPathParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let changed =
            tags::sync_header_file(&resolved).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": resolved, "changed": changed }))
    }

    #[tool(description = "为笔记添加标签并同步头部标签区")]
    fn tag_add(
        &self,
        Parameters(TagAddParams { path, tags }): Parameters<TagAddParams>,
    ) -> ToolOut {
        let cfg = load_cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let added = tags::add_tags(&resolved, &tags).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": resolved, "added": added }))
    }

    #[tool(description = "用配置的编辑器打开笔记（发起人工编辑；stdio 会话下仅适用于能独立开窗的编辑器，如 GUI 编辑器）")]
    fn open(&self, Parameters(TagPathParams { path }): Parameters<TagPathParams>) -> ToolOut {
        let cfg = load_cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let child = Command::new(&cfg.editor)
            .arg(&resolved)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| tool_err(format!("启动编辑器 {} 失败: {e}", cfg.editor)))?;
        ok_json(&json!({
            "path": resolved,
            "editor": cfg.editor,
            "pid": child.id()
        }))
    }
}

#[tool_handler(
    name = "anm-mcp",
    instructions = "anm 笔记系统记忆总线：按标签/目录/内容检索笔记，写入 inbox，维护标签。所有 path/dir 参数仅在笔记库根目录内有效。"
)]
impl ServerHandler for AnmServer {}

// ---------------------------------------------------------------------------
// 入口：默认按配置启动（本地 HTTP），CLI 标志可覆盖
// ---------------------------------------------------------------------------

enum Mode {
    Stdio,
    Http { host: String, port: u16 },
}

/// 解析命令行与配置，决定启动模式。
///
/// 优先级：显式 CLI 标志（`--stdio` / `--http` / `--host` / `--port`）
/// > 配置文件 `[mcp]` 段 > 默认（本地 HTTP `127.0.0.1:17371`）。
fn resolve_mode(cli: &[String], cfg: Option<&Config>) -> Result<Mode> {
    let mut force_http = false;
    let mut force_stdio = false;
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut i = 0;
    while i < cli.len() {
        match cli[i].as_str() {
            "--http" => force_http = true,
            "--stdio" => force_stdio = true,
            "--host" => {
                i += 1;
                host = cli.get(i).cloned();
                if host.is_none() {
                    anyhow::bail!("--host 缺少地址参数");
                }
            }
            "--port" => {
                i += 1;
                let p = cli
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--port 缺少端口参数"))?;
                port = Some(p.parse().map_err(|_| anyhow::anyhow!("端口非法: {p}"))?);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => anyhow::bail!("未知参数: {other}（--help 查看用法）"),
        }
        i += 1;
    }

    let mcp = cfg.map(|c| c.mcp.clone()).unwrap_or_default();
    // --host / --port 是 HTTP 专用参数，给出即视为要求 HTTP
    let want_http = force_http || host.is_some() || port.is_some();
    let mode = if force_stdio {
        Mode::Stdio
    } else if want_http || mcp.transport == McpTransport::Http {
        Mode::Http {
            host: host.unwrap_or(mcp.host),
            port: port.unwrap_or(mcp.port),
        }
    } else {
        Mode::Stdio
    };
    Ok(mode)
}

fn print_usage() {
    println!(
        "anm-mcp {}\n\
         用法:\n\
         \x20 anm-mcp                      按配置启动 MCP server（默认本地 HTTP：127.0.0.1:17371，端点 /mcp）\n\
         \x20 anm-mcp --stdio              强制 stdio 传输（供 Claude Desktop / Cursor / opencode 等 spawn）\n\
         \x20 anm-mcp --http [--host H] [--port P]\n\
         \x20                              强制 HTTP 传输，并可覆盖绑定地址 / 端口\n\
         \x20 anm-mcp --help               显示本帮助\n\
         配置: ~/.anm/config.toml 的 [mcp] 段（先运行 `anm init <笔记库根目录>`）\n\
         \x20   transport = \"http\" | \"stdio\"   # 默认 http\n\
         \x20   host = \"127.0.0.1\" / port = 17371  # 默认绑定",
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli: Vec<String> = std::env::args().skip(1).collect();
    // 配置读取失败（未 anm init）时按默认配置启动；工具调用会给出可操作提示
    let cfg = Config::load().ok();
    match resolve_mode(&cli, cfg.as_ref())? {
        Mode::Stdio => run_stdio().await,
        Mode::Http { host, port } => run_http(&host, port).await,
    }
}

/// stdio 传输：标准输入/输出上跑 JSON-RPC，直到连接关闭
async fn run_stdio() -> Result<()> {
    let server = AnmServer.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

/// Streamable HTTP 传输：POST /mcp 收发请求，GET /mcp 开 SSE 流
async fn run_http(host: &str, port: u16) -> Result<()> {
    let mut config = StreamableHttpServerConfig::default();
    // 简单请求-响应直接回 application/json，减少 SSE 开销
    config.json_response = true;
    config.allowed_hosts = vec![
        format!("{host}:{port}"),
        host.to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ];
    let service: StreamableHttpService<AnmServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(AnmServer::default()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    let app = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind((host, port))
        .await
        .with_context(|| format!("绑定 {host}:{port} 失败（端口被占用？）"))?;
    let addr = listener.local_addr()?;
    println!("anm-mcp: Streamable HTTP 已启动 → http://{addr}/mcp");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// 构造一个指向临时笔记库的 Config（不触碰真实 ~/.anm）
    fn test_config(name: &str) -> Config {
        let base = std::env::temp_dir().join(format!("anm-mcp-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        let root = base.join("notes");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        Config {
            home: home.clone(),
            config_path: home.join("config.toml"),
            root: root.clone(),
            editor: "true".to_string(),
            skatch: root.join("skatch.md"),
            index_path: home.join("index.jsonl"),
            mcp: anm_core::config::McpConfig::default(),
        }
    }

    #[test]
    fn tool_registry_is_complete_and_valid() {
        let tools = AnmServer::tool_router().list_all();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        let expected = [
            "ls_dirs",
            "list_tags",
            "find_tag",
            "search",
            "search_content",
            "read_note",
            "list",
            "recent",
            "new",
            "write_inbox",
            "tag_sync",
            "tag_add",
            "open",
        ];
        for name in expected {
            assert!(names.contains(&name.to_string()), "缺少工具 {name}");
        }
        // 无重复
        let set: HashSet<&String> = names.iter().collect();
        assert_eq!(set.len(), names.len(), "工具名重复");
        // 每个工具都有描述与合法 JSON Schema
        for t in &tools {
            assert!(t.description.is_some(), "{} 缺描述", t.name);
            let schema = t.schema_as_json_value();
            assert!(schema.get("type").is_some(), "{} schema 非法", t.name);
        }
    }

    #[tokio::test]
    async fn in_process_handshake_and_list_tools() -> anyhow::Result<()> {
        // 进程内双工传输：一端跑服务端，一端跑 rmcp 客户端，验证握手 + tools/list
        let (server_rw, client_rw) = tokio::io::duplex(1 << 20);
        let server = tokio::spawn(async move {
            let running = AnmServer.serve(server_rw).await.unwrap();
            let _ = running.waiting().await;
        });
        let client = ().serve(client_rw).await?;
        let listed = client.peer().list_tools(Default::default()).await?;
        assert_eq!(listed.tools.len(), 13);
        let names: HashSet<&str> = listed.tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains("read_note"));
        assert!(names.contains("search_content"));
        // 关闭客户端连接，服务端 waiting() 随之返回
        drop(client);
        let _ = server.await?;
        Ok(())
    }

    // ---- 启动模式解析（默认本地 HTTP，CLI 覆盖配置） ----

    #[test]
    fn default_mode_is_local_http() {
        let cfg = test_config("mode-default");
        match resolve_mode(&[], Some(&cfg)).unwrap() {
            Mode::Http { host, port } => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 17371);
            }
            Mode::Stdio => panic!("默认应为本地 HTTP"),
        }
    }

    #[test]
    fn stdio_flag_forces_stdio() {
        let cfg = test_config("mode-stdio-flag");
        assert!(matches!(resolve_mode(&["--stdio".into()], Some(&cfg)).unwrap(), Mode::Stdio));
        // 无配置时 --stdio 同样生效
        assert!(matches!(resolve_mode(&["--stdio".into()], None).unwrap(), Mode::Stdio));
    }

    #[test]
    fn config_stdio_is_respected() {
        let mut cfg = test_config("mode-config-stdio");
        cfg.mcp.transport = McpTransport::Stdio;
        assert!(matches!(resolve_mode(&[], Some(&cfg)).unwrap(), Mode::Stdio));
        // --http 覆盖配置中的 stdio
        assert!(matches!(resolve_mode(&["--http".into()], Some(&cfg)).unwrap(), Mode::Http { .. }));
    }

    #[test]
    fn host_port_override_config() {
        let mut cfg = test_config("mode-override");
        cfg.mcp.port = 9999;
        cfg.mcp.host = "0.0.0.0".to_string();
        // 显式 --host / --port 覆盖配置
        match resolve_mode(&["--host".into(), "10.0.0.1".into(), "--port".into(), "8080".into()], Some(&cfg)).unwrap() {
            Mode::Http { host, port } => {
                assert_eq!(host, "10.0.0.1");
                assert_eq!(port, 8080);
            }
            Mode::Stdio => panic!("应为 HTTP"),
        }
        // 无 CLI 覆盖时用配置值
        match resolve_mode(&[], Some(&cfg)).unwrap() {
            Mode::Http { host, port } => {
                assert_eq!(host, "0.0.0.0");
                assert_eq!(port, 9999);
            }
            Mode::Stdio => panic!("应为 HTTP"),
        }
    }

    #[test]
    fn unknown_flag_errors() {
        assert!(resolve_mode(&["--bogus".into()], None).is_err());
        assert!(resolve_mode(&["--host".into()], None).is_err());
    }
}
