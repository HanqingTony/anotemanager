//! IPC 协议：anm-core 服务与三个应用（anm / anw / anm-win-tray）之间的通信协议。
//!
//! - 传输：TCP + JSON 行（每个连接只处理一个请求，请求一行、响应一行）；
//! - 协议类型定义在 anm-core 的 lib 中，服务端（`server` 模块）与客户端
//!   （anm-cli 的 `client` 模块）共用同一份事实，避免两边各写一套结构；
//! - 请求经 `#[serde(tag = "cmd", content = "params")]` 序列化为
//!   `{"cmd": "...", "params": {...}}` 形式。

use serde::{Deserialize, Serialize};

/// IPC 请求：一个命令对应 anm-core 的一个确定性原语（现场扫描 / 低风险写入）。
///
/// 写入类命令（`TagMoveTop` / `TagAdd` / `InboxAppend`）只覆盖 readme §6
/// 允许的"新增/低风险追加"范围；对已有内容的修改/删除不通过 IPC 暴露。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "params", rename_all = "snake_case")]
pub enum Request {
    /// 列出笔记系统的一级目录（浏览入口）
    Dirs,
    /// 一级目录及其直接笔记的总览（聚合查询，供托盘覆盖层等一次取全）
    Overview,
    /// 任意目录（根目录内）的总览：直接笔记 + 直接子目录（托盘临时子卡片用）
    OverviewDir { dir: String },
    /// 按标签查找笔记：`tags` 为标签名数组（不含 @ 前缀），任一命中
    FindTag { tags: Vec<String> },
    /// 按标题 / 文件名关键字查找笔记（子串匹配，大小写不敏感）
    Search { keyword: String },
    /// 列出系统中出现的所有标签（去重排序）
    Tags,
    /// 将笔记中已识别的标签行移动到文档开头（纯位置整理）
    TagMoveTop { path: String },
    /// 为笔记新增标签：仅在开头标签区追加不存在的标签行
    TagAdd { path: String, tags: Vec<String> },
    /// 向默认 skatch.md 追加内容（inbox 入闸）
    InboxAppend { text: String },
    /// 读取一篇笔记的完整内容（托盘内置编辑器用；路径白名单校验）
    ReadNote { path: String },
    /// 写入一篇笔记的完整内容（托盘内置编辑器保存用；**人机通道**，
    /// MCP 不暴露此命令——AI 写入自主权原则不受影响）
    WriteNote { path: String, content: String },
    /// 在目录下新建一篇笔记（标题清洗为安全文件名，绝不覆盖已有文件；
    /// 托盘卡片标题行「+」按钮用；MCP 侧已有同名 tool）
    CreateNote { dir: String, title: String },
    /// 同目录内重命名一篇笔记（托盘编辑器改名用；**人机通道**，MCP 不暴露）
    RenameNote { from: String, to: String },
    /// skatch 总览：默认 inbox 文件的段落列表（空行分隔；托盘 skatch 卡片用）
    Skatch,
    /// 跨目录移动笔记文件（托盘文件行拖动用；**人机通道**，MCP 不暴露）
    MoveNote { from: String, to_dir: String },
    /// 从 skatch 抽取段落为独立文件（托盘把段落拖到目录卡片时用；
    /// **人机通道**，MCP 不暴露）
    SkatchExtract { dir: String, index: usize },
    /// 把笔记文件内容并入 skatch 末尾并删除原文件（托盘把文件拖到
    /// skatch 卡片时用；**人机通道**，MCP 不暴露）
    SkatchInsert { from: String },
}
/// IPC 信封：请求 + 可选访问令牌（服务端配置 `[server] token` 后校验）。
///
/// 传输层保持"一行一个消息"；客户端在信封里携带令牌，服务端校验。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// 访问令牌（`[server] token` 配置后必填；未配置时忽略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// 请求本体
    pub request: Request,
}

/// IPC 响应：`ok` 为 true 时 `data` 携带结果；为 false 时 `error` 携带错误描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// 是否成功
    pub ok: bool,
    /// 成功时的结果数据（JSON 值，随命令而异）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// 失败时的错误描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    /// 构造一个成功响应。
    pub fn ok(data: serde_json::Value) -> Response {
        Response {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    /// 构造一个失败响应。
    pub fn err(error: impl Into<String>) -> Response {
        Response {
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 请求各变体序列化/反序列化往返一致（服务端与客户端依赖同一份格式）。
    #[test]
    fn request_round_trips() {
        let cases = [
            Request::Dirs,
            Request::Overview,
            Request::OverviewDir {
                dir: "idea/sub".into(),
            },
            Request::FindTag {
                tags: vec!["ai".into(), "翻译".into()],
            },
            Request::Search {
                keyword: "postgres".into(),
            },
            Request::Tags,
            Request::TagMoveTop {
                path: "idea/a.md".into(),
            },
            Request::TagAdd {
                path: "idea/a.md".into(),
                tags: vec!["rust".into()],
            },
            Request::InboxAppend {
                text: "明天检查备份".into(),
            },
            Request::ReadNote {
                path: "idea/a.md".into(),
            },
            Request::WriteNote {
                path: "idea/a.md".into(),
                content: "# A\n".into(),
            },
            Request::CreateNote {
                dir: "idea".into(),
                title: "新笔记".into(),
            },
            Request::RenameNote {
                from: "idea/a.md".into(),
                to: "idea/b.md".into(),
            },
            Request::Skatch,
            Request::MoveNote {
                from: "idea/a.md".into(),
                to_dir: "ref".into(),
            },
            Request::SkatchExtract {
                dir: "idea".into(),
                index: 0,
            },
            Request::SkatchInsert {
                from: "idea/a.md".into(),
            },
        ];
        for req in cases {
            let json = serde_json::to_string(&req).unwrap();
            let back: Request = serde_json::from_str(&json).unwrap();
            assert_eq!(req, back, "往返不一致: {json}");
        }
    }

    /// 信封（令牌 + 请求）往返一致。
    #[test]
    fn envelope_round_trips() {
        let env = Envelope {
            token: Some("secret".into()),
            request: Request::Dirs,
        };
        let json = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
        // 无令牌时省略字段
        let json = serde_json::to_string(&Envelope { token: None, request: Request::Tags }).unwrap();
        assert!(!json.contains("token"));
    }

    /// 响应序列化格式：成功/失败各带对应字段。
    #[test]
    fn response_shapes() {
        let ok_resp = Response::ok(serde_json::json!({"n": 1}));
        let s = serde_json::to_string(&ok_resp).unwrap();
        assert!(s.contains("\"ok\":true"));
        assert!(s.contains("\"data\""));

        let err_resp = Response::err("出错了");
        let s = serde_json::to_string(&err_resp).unwrap();
        assert!(s.contains("\"ok\":false"));
        assert!(s.contains("\"error\":\"出错了\""));
    }
}
