#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 诊断：panic 写日志文件（GUI 子系统无控制台）
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::write("C:\\Users\\hanqi\\anm-tauri\\panic.log", format!("{info}\n"));
    }));
    anm_tauri_lib::run()
}
