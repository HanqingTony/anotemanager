//! anm-core：anm 的核心库与常驻服务（一核心三应用中的"核心"）。
//!
//! 本 crate 提供两部分：
//! - **lib**：确定性业务逻辑（config / tags / query / path / notes / inbox / tree）
//!   与 IPC 协议类型（protocol）——逻辑层不感知场景概念（readme §12），
//!   查询默认现场扫描、不维护任何持久索引（readme §10）；
//! - **bin（anm-core）**：常驻服务外壳，内嵌 lib 逻辑，提供文件监听、
//!   IPC 端点（供 anm / anw / anm-win-tray 三个应用）与内置 MCP server
//!   （HTTP 常驻端点 + stdio 会话，供 AI agent）。
//!
//! 分层原则：所有确定性逻辑在本 crate 的 lib 中；服务外壳（main / server /
//! mcp / watch 模块）只做接口适配，不包含业务判断。

pub mod config;
pub mod inbox;
pub mod notes;
pub mod path;
pub mod protocol;
pub mod query;
pub mod tags;
pub mod tree;

pub use config::{Config, McpConfig, McpTransport, ServerConfig};
pub use protocol::{Request, Response};
