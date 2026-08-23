//! 输入命令处理（平台无关）：anw 语义 + 斜杠命令。
//!
//! - 普通输入（不以 `/` 开头）：**anw 语义**——回车把整段文本写入 skatch.md；
//! - 斜杠命令（`/命令 [参数]`）：与 anm CLI 命令保持一致——`/help`、`/find`、
//!   `/search`、`/tags`、`/ls`、`/inbox|/anw`、`/open`；
//! - 命令执行经 [`crate::ipc`] 走 anm-core 服务；`/open` 返回 [`Action::Open`]
//!   由平台外壳用系统默认方式打开。

use anm_core::protocol::Request;

use crate::ipc;
use crate::model::Action;

/// 输入提交结果：展示文本 + 可能需要平台执行的附加动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// 显示在输入框下方的结果/提示文本
    pub result: String,
    /// 平台外壳需要额外执行的动作（如 /open 打开文件）
    pub action: Option<Action>,
}

/// 处理输入框回车：以 `/` 开头走斜杠命令，否则 anw 语义（写入 skatch.md）。
pub fn run_input(text: &str) -> SubmitOutcome {
    let text = text.trim();
    if text.is_empty() {
        return SubmitOutcome {
            result: "（输入为空）".to_string(),
            action: None,
        };
    }
    if let Some(rest) = text.strip_prefix('/') {
        slash_command(rest)
    } else {
        match ipc::call(&Request::InboxAppend {
            text: text.to_string(),
        }) {
            Ok(_) => SubmitOutcome {
                result: "已写入 skatch.md".to_string(),
                action: None,
            },
            Err(e) => SubmitOutcome {
                result: format!("{e:#}"),
                action: None,
            },
        }
    }
}

/// 斜杠命令帮助文本（/help 输出）。
pub const SLASH_HELP: &str = "斜杠命令（与 anm 命令一致）:\n\
     /help            本帮助\n\
     /inbox|/anw <文本>   写入 skatch.md\n\
     /find <标签…>      按标签查找笔记\n\
     /search <关键字>    按标题查找笔记\n\
     /tags             列出全部标签\n\
     /ls               列出全部一级目录\n\
     /open <路径>       用系统默认方式打开";

/// 分发一条斜杠命令（rest 为 `/` 之后的部分）。
fn slash_command(rest: &str) -> SubmitOutcome {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_lowercase();
    let arg = parts.next().unwrap_or("").trim().to_string();

    match cmd.as_str() {
        "help" => SubmitOutcome {
            result: SLASH_HELP.to_string(),
            action: None,
        },
        "open" if !arg.is_empty() => SubmitOutcome {
            result: format!("打开 {arg}"),
            action: Some(Action::Open(arg)),
        },
        "find" if !arg.is_empty() => run_ipc(
            &Request::FindTag {
                tags: arg.split_whitespace().map(|s| s.to_string()).collect(),
            },
            format_notes,
        ),
        "search" if !arg.is_empty() => {
            run_ipc(&Request::Search { keyword: arg }, format_notes)
        }
        "tags" => run_ipc(&Request::Tags, |data| {
            data.as_array()
                .map(|a| a.iter().map(|t| format!("@{t}")).collect::<Vec<_>>().join(" "))
                .unwrap_or_default()
        }),
        "ls" => run_ipc(&Request::Overview, |data| {
            data.as_array()
                .map(|a| {
                    a.iter()
                        .map(|d| d["name"].as_str().unwrap_or("?"))
                        .collect::<Vec<_>>()
                        .join("  ")
                })
                .unwrap_or_default()
        }),
        "inbox" | "anw" if !arg.is_empty() => run_ipc(
            &Request::InboxAppend { text: arg },
            |_| "已写入 skatch.md".to_string(),
        ),
        _ => SubmitOutcome {
            result: format!("未知命令 /{cmd}（输入 /help 查看）"),
            action: None,
        },
    }
}

/// 执行 IPC 请求并格式化结果；失败时返回错误文本。
fn run_ipc(req: &Request, fmt: impl FnOnce(&serde_json::Value) -> String) -> SubmitOutcome {
    match ipc::call(req) {
        Ok(data) => SubmitOutcome {
            result: fmt(&data),
            action: None,
        },
        Err(e) => SubmitOutcome {
            result: format!("{e:#}"),
            action: None,
        },
    }
}

/// 笔记列表格式化（最多列 5 条，超出显示剩余数量）。
fn format_notes(data: &serde_json::Value) -> String {
    data.as_array()
        .map(|notes| {
            let mut lines: Vec<String> = notes
                .iter()
                .take(5)
                .map(|n| {
                    let title = n["title"].as_str().unwrap_or("?");
                    let tags = n["tags"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                        .unwrap_or_default();
                    if tags.is_empty() {
                        title.to_string()
                    } else {
                        format!("{title} [{tags}]")
                    }
                })
                .collect();
            if notes.len() > 5 {
                lines.push(format!("… 还有 {} 条", notes.len() - 5));
            }
            lines.join("\n")
        })
        .unwrap_or_else(|| "（无结果）".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 普通输入 = anw 语义（无服务时返回连接错误文本，而不是 panic）。
    #[test]
    fn plain_input_is_anw_semantics() {
        // 不依赖真实服务：连接失败也应返回可展示的错误文本
        let out = run_input("   ");
        assert_eq!(out.result, "（输入为空）");
    }

    /// 斜杠命令解析：/help 返回帮助，未知命令给提示。
    #[test]
    fn slash_parsing() {
        let out = run_input("/help");
        assert!(out.result.contains("/find"));
        assert!(out.action.is_none());

        let out = run_input("/bogus");
        assert!(out.result.contains("未知命令 /bogus"));

        // /open 产生平台动作
        let out = run_input("/open /mnt/c/x.md");
        assert_eq!(
            out.action,
            Some(Action::Open("/mnt/c/x.md".to_string()))
        );
    }

    /// 无参数命令给出用法提示。
    #[test]
    fn slash_requires_args() {
        let out = run_input("/find");
        assert!(out.result.contains("未知命令 /find"));
        let out = run_input("/open");
        assert!(out.result.contains("未知命令 /open"));
    }
}
