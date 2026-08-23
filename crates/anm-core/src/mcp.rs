//! anm-core 内置的 MCP server（官方 rmcp 实现），把 lib 的能力暴露给 AI agent。
//!
//! 对 agent 而言这是访问笔记系统的记忆总线（取指-加载通道），而非单纯的检索接口。
//! MCP 是 anm-core 服务的一个功能（一核心三应用中的"核心"自带的 agent 前端）：
//! - HTTP 模式：随服务常驻（默认 `127.0.0.1:17371/mcp`），客户端直接连接；
//! - stdio 模式：`anm-core --stdio`，被 MCP 客户端（Claude Desktop / Cursor /
//!   opencode / DeepSeek Harness 等）每次会话 spawn，会话结束进程退出。
//!
//! ## 传输（默认本地 HTTP，可在 `~/.anm/config.toml` 的 `[mcp]` 段配置）
//! - `anm-core`：随服务启动 **Streamable HTTP**（`127.0.0.1:17371`，端点 `/mcp`）；
//!   绑定的 host/port 与传输方式来自配置 `[mcp]` 段，CLI 标志（`--stdio` / `--http` /
//!   `--host` / `--port`）可覆盖。
//! - **stdio**：`anm-core --stdio`（供 Claude Desktop / Cursor / opencode 等 spawn，零网络依赖）
//!
//! ## 安全边界（与设计文档一致）
//! - 路径白名单：所有 path / dir 参数经 `anm_core::path` 校验，仅允许笔记系统根目录内；
//! - 只读优先：写操作仅 `new` / `write_inbox` / `tag_add` / `tag_move_top`（均为
//!   新增/低风险追加类，符合 readme §6 的 AI 写入自主权原则），不暴露 shell；
//! - `read_note` 限长截断，避免 agent 上下文被整库灌爆。

use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::{
    ErrorData as McpError, ServiceExt, ServerHandler,
    model::{
        CallToolResult, ContentBlock, ListResourcesResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
        ResourceContents, ResultType,
    },
    schemars, tool, tool_handler, tool_router,
    handler::server::wrapper::Parameters,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
    RoleServer,
};
use serde::Serialize;
use serde_json::json;

use anm_core::{
    config::{Config, McpTransport}, inbox, notes, path, query, tags, tree,
};

/// `read_note` 默认截断长度（字符数）
const DEFAULT_READ_LIMIT: usize = 8000;

/// 通用事实资源的 URI（readme §13"个人常识性事实"，常驻上下文、默认只读）。
///
/// 内容来自笔记库根目录下人工维护的 `.agentspace/fact.md`；永远以当前
/// 文件内容为准（作者决定：不做新鲜度/过期标记机制）。
const FACTS_RESOURCE_URI: &str = "anm://facts";

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
///
/// 持有会话配置快照（stdio 会话 / HTTP 会话创建时各加载一次），工具与
/// 资源共用同一份配置，避免每次调用都重新读 `~/.anm/config.toml`。
#[derive(Debug, Clone)]
pub struct AnmServer {
    /// 配置快照；`None` 表示尚未 `anm init`（工具调用会给出可操作提示）
    cfg: Option<Config>,
}

impl Default for AnmServer {
    /// 默认按当前 `~/.anm/config.toml` 加载快照；未初始化时为 `None`。
    fn default() -> Self {
        Self {
            cfg: Config::load().ok(),
        }
    }
}

impl AnmServer {
    /// 返回会话配置快照；未初始化时给出可操作的提示。
    fn cfg(&self) -> Result<Config, CallToolResult> {
        self.cfg.clone().ok_or_else(|| {
            tool_err("未找到配置，请先在笔记系统侧运行 `anm init <笔记库根目录>`")
        })
    }
}

#[tool_router]
impl AnmServer {
    #[tool(description = "列出笔记系统的一级目录（浏览入口）")]
    fn ls_dirs(&self, Parameters(NoParams {}): Parameters<NoParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let dirs = tree::list_top_dirs(&cfg.root).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&dirs)
    }

    #[tool(description = "列出系统中所有标签")]
    fn list_tags(&self, Parameters(NoParams {}): Parameters<NoParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let tags = query::all_tags(&cfg.root).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&tags)
    }

    #[tool(description = "按标签查找笔记。tags 为标签名数组（不含 @ 前缀），任一命中")]
    fn find_tag(&self, Parameters(FindTagParams { tags }): Parameters<FindTagParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let notes = query::find_by_tag(&cfg.root, &tags).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "按标题 / 文件名关键字查找笔记（子串匹配，大小写不敏感）")]
    fn search(&self, Parameters(KeywordParams { keyword }): Parameters<KeywordParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let notes =
            query::find_by_title(&cfg.root, &keyword).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "全文搜索笔记正文。返回命中片段（snippet）与命中次数（score），按 score 降序，limit 限制条数")]
    fn search_content(
        &self,
        Parameters(ContentSearchParams { keyword, limit }): Parameters<ContentSearchParams>,
    ) -> ToolOut {
        let cfg = self.cfg()?;
        let hits = query::search_content(&cfg.root, &keyword, limit.unwrap_or(20))
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&hits)
    }

    #[tool(description = "读取一篇笔记的完整内容（限长截断，防止上下文爆炸）。path 可为相对笔记库根的路径或绝对路径；limit 为字符数上限")]
    fn read_note(
        &self,
        Parameters(ReadNoteParams { path, limit }): Parameters<ReadNoteParams>,
    ) -> ToolOut {
        let cfg = self.cfg()?;
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
        let cfg = self.cfg()?;
        let notes = query::list_in_dir(&cfg.root, dir.as_deref().unwrap_or("."))
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "最近修改的笔记（按最后修改时间倒序），n 为条数（默认 10）")]
    fn recent(&self, Parameters(RecentParams { n }): Parameters<RecentParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let notes =
            query::recent(&cfg.root, n.unwrap_or(10)).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&notes)
    }

    #[tool(description = "在笔记系统内新建一篇笔记：dir 为相对根目录的已存在目录，title 用作文件名，content 可选")]
    fn new(&self, Parameters(NewNoteParams { dir, title, content }): Parameters<NewNoteParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let content = content.as_deref().unwrap_or("");
        let created = notes::create_note(&cfg.root, &dir, &title, content)
            .map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": created, "created": true }))
    }

    #[tool(description = "向默认 skatch.md（inbox 入闸缓冲）写入内容，适合记录临时想法、待办、冲动")]
    fn write_inbox(&self, Parameters(InboxParams { text }): Parameters<InboxParams>) -> ToolOut {
        let cfg = self.cfg()?;
        inbox::append(&cfg.skatch, &text).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "written": true, "skatch": cfg.skatch }))
    }

    #[tool(description = "将笔记中已识别的标签行移动到文档开头（纯位置整理：不合并、不排序、不改写标签内容与语义）。自主状态下对已有标签唯一允许的操作")]
    fn tag_move_top(&self, Parameters(TagPathParams { path }): Parameters<TagPathParams>) -> ToolOut {
        let cfg = self.cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let changed =
            tags::move_tag_lines_to_top_file(&resolved).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": resolved, "changed": changed }))
    }

    #[tool(description = "为笔记新增标签：仅在文档开头的标签区追加不存在的标签行，不改动任何已有标签行")]
    fn tag_add(
        &self,
        Parameters(TagAddParams { path, tags }): Parameters<TagAddParams>,
    ) -> ToolOut {
        let cfg = self.cfg()?;
        let resolved = resolve_note_arg(&cfg, &path)?;
        let added = tags::add_tags(&resolved, &tags).map_err(|e| tool_err(e.to_string()))?;
        ok_json(&json!({ "path": resolved, "added": added }))
    }

    #[tool(description = "用配置的编辑器打开笔记（发起人工编辑；stdio 会话下仅适用于能独立开窗的编辑器，如 GUI 编辑器）")]
    fn open(&self, Parameters(TagPathParams { path }): Parameters<TagPathParams>) -> ToolOut {
        let cfg = self.cfg()?;
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
    name = "anm-core",
    instructions = "anm 笔记系统记忆总线（anm-core 内置）：按标签/目录/内容检索笔记，写入 inbox，新增标签、整理标签行位置。所有 path/dir 参数仅在笔记库根目录内有效；对已有内容的修改/删除仅在作者显式指令下进行。开始工作时先读取资源 anm://facts（通用事实：家庭设备连接方式、VPS 凭据、当前项目等，人工维护、默认只读，永远以当前内容为准）。"
)]
impl ServerHandler for AnmServer {
    /// 列出本服务器提供的全部资源（当前只有一个：通用事实）。
    ///
    /// 资源是 MCP 协议里"常驻上下文"的标准通道（readme §13）：客户端在
    /// 会话开始时枚举并读取，agent 无需专门发起查询即可获得通用事实。
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            result_type: Some(ResultType::COMPLETE),
            meta: None,
            next_cursor: None,
            ttl_ms: None,
            cache_scope: None,
            resources: vec![Resource::new(FACTS_RESOURCE_URI, "通用事实").with_description(
                "家庭网络设备连接方式、VPS 凭据、当前项目等人工维护的通用事实，来自笔记库 .agentspace/fact.md；默认只读，永远以当前文件内容为准。",
            ).with_mime_type("text/markdown")],
        })
    }

    /// 读取 `anm://facts`：返回笔记库 `.agentspace/fact.md` 的当前内容。
    ///
    /// - 未知 URI → `resource_not_found`；
    /// - 未初始化配置 / 文件缺失 → 带原因的 `resource_not_found`；
    /// - 读取永远现场进行，不缓存（作者决定：以当前 fact.md 为准）。
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri != FACTS_RESOURCE_URI {
            return Err(McpError::resource_not_found(request.uri, None));
        }
        let cfg = self.cfg.clone().ok_or_else(|| {
            McpError::resource_not_found(
                FACTS_RESOURCE_URI,
                Some(json!({"hint": "未找到配置，请先运行 `anm init <笔记库根目录>`"})),
            )
        })?;
        let facts_path = cfg.root.join(".agentspace").join("fact.md");
        let text = std::fs::read_to_string(&facts_path).map_err(|e| {
            McpError::resource_not_found(
                FACTS_RESOURCE_URI,
                Some(json!({"path": facts_path, "error": e.to_string()})),
            )
        })?;
        Ok(ReadResourceResponse::from(ReadResourceResult::new(vec![
            ResourceContents::text(text, FACTS_RESOURCE_URI).with_mime_type("text/markdown"),
        ])))
    }
}

// ---------------------------------------------------------------------------
// 入口：默认按配置启动（本地 HTTP），CLI 标志可覆盖
// ---------------------------------------------------------------------------

/// MCP 的启动形态：stdio 单会话，或随完整服务常驻的 HTTP 端点。
pub enum Mode {
    /// 只跑一个 stdio MCP 会话（被客户端 spawn 时使用）
    Stdio,
    /// 作为完整服务的一部分常驻 HTTP 端点
    Http { host: String, port: u16 },
}

/// 解析命令行与配置，决定 MCP 的启动形态（供 main 分发）。
///
/// 优先级：显式 CLI 标志（`--stdio` / `--http` / `--host` / `--port`）
/// > 配置文件 `[mcp]` 段 > 默认（本地 HTTP `127.0.0.1:17371`）。
pub fn resolve_mode(cli: &[String], cfg: Option<&Config>) -> Result<Mode> {
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

/// 打印 anm-core 的 MCP 用法帮助（`--help` 时输出后退出）。
fn print_usage() {
    println!(
        "anm-core {}\n\
         用法:\n\
         \x20 anm-core                       启动完整服务（文件监听 + IPC + MCP HTTP，端点 /mcp）\n\
         \x20 anm-core --stdio               只跑一个 MCP stdio 会话（供 Claude Desktop / Cursor / opencode 等 spawn）\n\
         \x20 anm-core --http [--host H] [--port P]\n\
         \x20                               覆盖 MCP HTTP 的绑定地址 / 端口后启动完整服务\n\
         \x20 anm-core --help                显示本帮助\n\
         配置: ~/.anm/config.toml 的 [mcp] 段（先运行 `anm init <笔记库根目录>`）\n\
         \x20   transport = \"http\" | \"stdio\"   # 默认 http\n\
         \x20   host = \"127.0.0.1\" / port = 17371  # 默认绑定",
        env!("CARGO_PKG_VERSION")
    );
}

/// stdio 传输：标准输入/输出上跑 JSON-RPC，直到连接关闭。
///
/// 由 `anm-core --stdio` 调用；MCP 客户端（Claude Desktop / Cursor /
/// opencode / DeepSeek Harness 等）每次会话 spawn 本进程。
pub async fn run_stdio() -> Result<()> {
    let server = AnmServer::default().serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}

/// Streamable HTTP 传输：POST /mcp 收发请求，GET /mcp 开 SSE 流。
///
/// 随完整服务常驻；`--host` / `--port` 可临时覆盖绑定地址。
pub async fn run_http(host: &str, port: u16) -> Result<()> {
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
    println!("anm-core: MCP HTTP 已启动 → http://{addr}/mcp");
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
        let base = std::env::temp_dir().join(format!("anm-core-mcp-test-{name}-{}", std::process::id()));
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
            mcp: anm_core::config::McpConfig::default(),
            server: anm_core::config::ServerConfig::default(),
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
            "tag_move_top",
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
            let running = AnmServer::default().serve(server_rw).await.unwrap();
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

    // ---- 通用事实资源（anm://facts） ----

    /// 构造一个带临时笔记库配置、且 `.agentspace/fact.md` 已就位的服务器。
    fn facts_server(name: &str, fact_content: &str) -> (AnmServer, Config) {
        let cfg = test_config(name);
        std::fs::create_dir_all(cfg.root.join(".agentspace")).unwrap();
        std::fs::write(cfg.root.join(".agentspace/fact.md"), fact_content).unwrap();
        let server = AnmServer {
            cfg: Some(cfg.clone()),
        };
        (server, cfg)
    }

    /// 通过进程内客户端走真实协议：resources/list 能列出，resources/read 能读到内容。
    #[tokio::test]
    async fn resources_list_and_read_facts() -> anyhow::Result<()> {
        let (server, cfg) = facts_server("resource", "# 通用事实\n\n- VPS: 1.2.3.4\n");
        let (server_rw, client_rw) = tokio::io::duplex(1 << 20);
        let server_task = tokio::spawn(async move {
            let running = server.serve(server_rw).await.unwrap();
            let _ = running.waiting().await;
        });
        let client = ().serve(client_rw).await?;

        // resources/list：正好一个资源，URI 正确
        let listed = client.peer().list_resources(None).await?;
        assert_eq!(listed.resources.len(), 1);
        assert_eq!(listed.resources[0].uri, FACTS_RESOURCE_URI);

        // resources/read：内容与 fact.md 一致（永远以当前文件为准）
        let read = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(FACTS_RESOURCE_URI))
            .await?;
        let contents = read.contents;
        assert_eq!(contents.len(), 1);
        match &contents[0] {
            ResourceContents::TextResourceContents { text, .. } => {
                assert!(text.contains("VPS: 1.2.3.4"));
            }
            other => panic!("应返回文本内容: {other:?}"),
        }

        // 未知 URI 报 resource_not_found
        let err = client
            .peer()
            .read_resource(ReadResourceRequestParams::new("anm://nope"))
            .await;
        assert!(err.is_err(), "未知 URI 应报错");

        drop(client);
        let _ = server_task.await?;
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
        Ok(())
    }

    /// fact.md 缺失时返回带原因的 resource_not_found，而不是崩溃。
    #[tokio::test]
    async fn resources_read_missing_file_errors() -> anyhow::Result<()> {
        let cfg = test_config("resource-missing");
        let server = AnmServer {
            cfg: Some(cfg.clone()),
        };
        let (server_rw, client_rw) = tokio::io::duplex(1 << 20);
        let server_task = tokio::spawn(async move {
            let running = server.serve(server_rw).await.unwrap();
            let _ = running.waiting().await;
        });
        let client = ().serve(client_rw).await?;

        let err = client
            .peer()
            .read_resource(ReadResourceRequestParams::new(FACTS_RESOURCE_URI))
            .await;
        assert!(err.is_err(), "fact.md 缺失应报错");

        drop(client);
        let _ = server_task.await?;
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
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
