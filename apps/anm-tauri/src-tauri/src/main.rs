// Windows GUI 子系统无控制台；Linux 下保留 stderr（panic 默认输出）
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    // 诊断：panic 写日志文件（exe 同目录 panic.log；Windows GUI 子系统无控制台）
    std::panic::set_hook(Box::new(|info| {
        let path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("panic.log")))
            .unwrap_or_else(|| std::path::PathBuf::from("panic.log"));
        let _ = std::fs::write(path, format!("{info}\n"));
    }));
    anm_tauri_lib::run()
}
