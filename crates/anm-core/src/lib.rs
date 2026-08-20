//! anm-core：anm 的核心库，实现全部基本功能。
//!
//! 分层原则：所有逻辑在 anm_core，CLI / MCP / daemon / tray 只做接口适配。

pub mod config;
pub mod inbox;
pub mod index;
pub mod query;
pub mod tags;
pub mod tree;

pub use config::Config;
pub use index::IndexEntry;
