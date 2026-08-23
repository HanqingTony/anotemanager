//! 文件监听：观察笔记目录变动。
//!
//! 非唯一入口原则（readme §10）：本模块只**观察**，不拦截、不修改任何外部
//! 写入；用户可随时用任何形式修改笔记目录。变动事件经 500ms 防抖后打印到
//! 服务日志——未来在此接入分诊触发 / 定时提醒等动作时，也只做"观察后的
//! 动作"，不改变"文件系统是唯一真相源"这一事实。

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};

/// 递归监听笔记目录，把变动事件（防抖合并后）打印到 stdout，直到进程退出。
///
/// - 内部使用 notify 的推荐 watcher（自带后台线程），事件经 mpsc 通道回传；
/// - 连续事件先睡 500ms 合并，避免一次批量写入（如 `anw` 连写、外部同步）
///   刷屏；
/// - 本函数阻塞运行（`for ev in rx` 直到通道关闭），由服务进程持有。
pub fn run(root: &Path) -> Result<()> {
    let (evt_tx, evt_rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = evt_tx.send(res);
        },
        NotifyConfig::default(),
    )
    .context("创建文件监听器失败")?;
    watcher
        .watch(root, RecursiveMode::Recursive)
        .with_context(|| format!("监听 {} 失败", root.display()))?;
    println!("anm-core: 正在监听 {}", root.display());

    for ev in evt_rx {
        match ev {
            Ok(event) => {
                thread::sleep(Duration::from_millis(500));
                for p in event.paths {
                    println!("anm-core: 变动 {}", p.display());
                }
            }
            Err(e) => eprintln!("anm-core: 监听事件错误: {e}"),
        }
    }
    Ok(())
}
