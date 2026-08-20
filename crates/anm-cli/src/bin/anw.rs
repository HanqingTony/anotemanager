//! anw：快速把后续参数写入默认 skatch.md（inbox 入口）。
//!
//! 用法：`anw 明天检查 postgres 备份`

use anm_core::{config::Config, inbox};

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

    if let Err(e) = inbox::append(&cfg.skatch, &text) {
        eprintln!("anw: {e:#}");
        std::process::exit(1);
    }
    println!("已写入 {}", cfg.skatch.display());
}
