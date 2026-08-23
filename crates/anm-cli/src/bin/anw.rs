//! anw：快速把后续参数写入默认 skatch.md（anm-core 服务的客户端应用之一）。
//!
//! 用法：`anw 明天检查 postgres 备份`
//!
//! 本应用不直接碰文件：经 IPC 请求常驻的 anm-core 服务向 skatch.md 追加
//! 内容（服务未启动时报错并提示）。

use anm_core::config::Config;
use anm_core::protocol::Request;

use anm_cli::client;

/// 程序入口：把命令行参数拼成一句话，经 IPC 请求 anm-core 服务追加到 skatch.md。
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let text = args.join(" ");

    if text.trim().is_empty() {
        eprintln!("anw: 用法: anw <内容>");
        std::process::exit(1);
    }

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("anw: {e:#}");
            std::process::exit(1);
        }
    };

    match client::call(&cfg, &Request::InboxAppend { text }) {
        Ok(data) => println!("已写入 {}", data["skatch"]),
        Err(e) => {
            eprintln!("anw: {e:#}");
            std::process::exit(1);
        }
    }
}
