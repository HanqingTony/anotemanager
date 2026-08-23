//! anm-cli：anm-core 服务的客户端库与三个应用中的两个命令行应用
//! （`anm` 主命令、`anw` 快速写入）。
//!
//! 一核心三应用架构下，本 crate 不包含任何业务逻辑：查询与写入全部经
//! [`client`] 模块走 IPC 转发给常驻的 anm-core 服务；`init` / `open` /
//! `completion` 等纯本地动作在应用侧直接完成。

pub mod client;
