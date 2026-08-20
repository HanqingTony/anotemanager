//! anm-mcp：MCP server，把 anm_core 的能力暴露给 AI agent。
//!
//! 协议：MCP（JSON-RPC 2.0）over stdio。
//! 对 agent 而言这是访问笔记系统的记忆总线（取指-加载通道），而非单纯的检索接口。

use std::io::{self, BufRead, Write};

use anyhow::Result;
use serde_json::{json, Value};

use anm_core::{config::Config, inbox, query, tags, tree};

/// MCP 协议版本
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 工具清单：(name, description, input_schema)
const TOOLS: &[(&str, &str, &str)] = &[
    (
        "ls_dirs",
        "列出笔记系统的一级目录",
        r#"{"type":"object","properties":{},"required":[]}"#,
    ),
    (
        "list_tags",
        "列出系统中所有标签",
        r#"{"type":"object","properties":{},"required":[]}"#,
    ),
    (
        "find_tag",
        "按标签查找笔记。tags 为标签名数组（不含 @ 前缀），任一命中",
        r#"{"type":"object","properties":{"tags":{"type":"array","items":{"type":"string"}}},"required":["tags"]}"#,
    ),
    (
        "search",
        "按标题 / 文件名关键字查找笔记",
        r#"{"type":"object","properties":{"keyword":{"type":"string"}},"required":["keyword"]}"#,
    ),
    (
        "read_note",
        "读取一篇笔记的完整内容",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    ),
    (
        "write_inbox",
        "向默认 skatch.md（inbox 入闸缓冲）写入内容，适合记录临时想法、待办、冲动",
        r#"{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}"#,
    ),
    (
        "tag_sync",
        "同步一篇笔记的头部标签区：把文档中的标签行统一维护到文件头部",
        r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#,
    ),
    (
        "tag_add",
        "为笔记添加标签并同步头部标签区",
        r#"{"type":"object","properties":{"path":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}},"required":["path","tags"]}"#,
    ),
];

fn main() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(resp) = handle(&msg) {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{}", serde_json::to_string(&resp)?);
            let _ = out.flush();
        }
    }
    Ok(())
}

/// 处理一条消息；返回需要回应的响应（Notification 返回 None）
fn handle(msg: &Value) -> Option<Value> {
    let method = msg.get("method")?.as_str()?;
    let id = msg.get("id");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "anm-mcp", "version": env!("CARGO_PKG_VERSION") }
            }
        })),
        "notifications/initialized" => None,
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/list" => {
            let tools: Vec<Value> = TOOLS
                .iter()
                .map(|(name, desc, schema)| {
                    json!({
                        "name": name,
                        "description": desc,
                        "inputSchema": serde_json::from_str::<Value>(schema).unwrap_or(Value::Null)
                    })
                })
                .collect();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools }
            }))
        }
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            let cfg = Config::load();
            let result = match cfg {
                Ok(c) => call_tool(name, &args, &c),
                Err(e) => tool_error(&format!("{e:#}\n提示：先在笔记系统侧运行 `anm init <根目录>`")),
            };
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            }))
        }
        _ => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": tool_error(&format!("未知方法: {method}"))
        })),
    }
}

/// 工具调用，返回 MCP tool result（content / isError）
fn call_tool(name: &str, args: &Value, cfg: &Config) -> Value {
    let args = if args.is_null() { &Value::Null } else { args };
    let out: Result<Value, String> = (|| {
        match name {
            "ls_dirs" => {
                let dirs = tree::list_top_dirs(&cfg.root).map_err(|e| e.to_string())?;
                Ok(json!(dirs))
            }
            "list_tags" => {
                let tags = query::all_tags(&cfg.root).map_err(|e| e.to_string())?;
                Ok(json!(tags))
            }
            "find_tag" => {
                let tags: Vec<String> = args
                    .get("tags")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let notes = query::find_by_tag(&cfg.root, &tags).map_err(|e| e.to_string())?;
                Ok(json!(notes))
            }
            "search" => {
                let keyword = args.get("keyword").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let notes = query::find_by_title(&cfg.root, &keyword).map_err(|e| e.to_string())?;
                Ok(json!(notes))
            }
            "read_note" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    return Err("缺少 path 参数".to_string());
                }
                let content = std::fs::read_to_string(path)
                    .map_err(|e| format!("读取 {path} 失败: {e}"))?;
                Ok(json!({ "path": path, "content": content }))
            }
            "write_inbox" => {
                let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
                inbox::append(&cfg.skatch, text).map_err(|e| e.to_string())?;
                Ok(json!({ "written": true, "skatch": cfg.skatch }))
            }
            "tag_sync" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                if path.is_empty() {
                    return Err("缺少 path 参数".to_string());
                }
                let changed =
                    tags::sync_header_file(std::path::Path::new(path)).map_err(|e| e.to_string())?;
                Ok(json!({ "path": path, "changed": changed }))
            }
            "tag_add" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let tags: Vec<String> = args
                    .get("tags")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                if path.is_empty() || tags.is_empty() {
                    return Err("需要 path 与 tags 参数".to_string());
                }
                let added = tags::add_tags(std::path::Path::new(path), &tags)
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "path": path, "added": added }))
            }
            _ => Err(format!("未知工具: {name}")),
        }
    })();

    match out {
        Ok(data) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&data).unwrap_or_default() }],
            "isError": false
        }),
        Err(e) => tool_error(&e),
    }
}

fn tool_error(msg: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": format!("错误: {msg}") }],
        "isError": true
    })
}
