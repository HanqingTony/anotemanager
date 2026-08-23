//! anm-tray-win：Windows 托盘薄壳（平台外壳之一）。
//!
//! 架构：逻辑全部在 `anm-tray-core`（跨平台共享），本 crate 只做
//! **Windows 外壳**——窗口/渲染/输入事件 → 调核心更新状态 → 执行 Action。
//! 未来的 anm-tray-wayland / anm-tray-android 只重写外壳，复用核心。
//!
//! 功能（与核心配合）：
//! - 单例常驻托盘 + 全局快捷键（Alt+Shift+Z）呼出覆盖层；
//! - 全屏半透明变暗层（逐像素合成）+ 纯色卡片环绕 + 居中输入框（anw 语义）；
//! - 卡片：点击打开/编辑、拖动记忆位置、滚轮滚动、子目录 → 临时子卡片；
//! - 输入框：斜杠命令（/help /find 等）；点击文本条目 → 内置临时编辑器。

// Linux 上 wslpath 仅被 cfg(windows) 的 win 模块引用，属"暂未使用"，
// 允许 dead_code 以免 Linux 构建产生噪音警告。
#![cfg_attr(not(windows), allow(dead_code))]

/// WSL 路径 → Windows 路径（仅 Windows 外壳需要；纯函数，Linux 上可单测）。
mod wslpath;

fn main() {
    #[cfg(windows)]
    win::run();
    #[cfg(not(windows))]
    {
        eprintln!("anm-tray-win 仅支持 Windows（Linux 下请用 GNU 路线交叉编译，见 feature.md）");
        std::process::exit(1);
    }
}

/// Windows 实现（仅在 windows 目标下编译）。
#[cfg(windows)]
mod win;
