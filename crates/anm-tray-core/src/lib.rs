//! anm-tray-core：托盘应用的**跨平台共享核心**（纯逻辑，无任何窗口系统依赖）。
//!
//! 架构（一核心三应用 × 多平台外壳）：
//! - 本 crate 承载与平台无关的一切：卡片布局/命中/拖动/滚动、状态模型、
//!   输入命令（anw + 斜杠命令）、IPC 客户端；
//! - 平台外壳 crate（anm-tray-win；未来 anm-tray-wayland / anm-tray-android）
//!   只做三件事：**渲染**（把状态画到屏幕上）、**输入事件 → 调核心**、
//!   **执行核心返回的 Action**（打开文件/显示隐藏窗口/切换编辑器等）。
//!
//! 这样 Android / Wayland 版本只需重写外壳，卡片、命令、协议全部复用；
//! 纯逻辑都可以在 Linux 上直接单元测试。

pub mod cards;
pub mod hotkey;
pub mod commands;
pub mod ipc;
pub mod model;

pub use cards::{Card, CardRow, Hit, LayoutParams, Rect};
pub use model::{Action, DragState, EditorState, TrayState};
