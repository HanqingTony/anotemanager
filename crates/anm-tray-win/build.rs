//! 构建脚本：为 Windows 目标嵌入应用图标（资源 id 1，`anm-win-tray` 窗口类与托盘共用）。
//!
//! - 只在构建 Windows 目标时执行（交叉编译时 host 是 Linux，因此用
//!   `CARGO_CFG_TARGET_OS` 环境变量判断，而不是 `cfg!(windows)`）；
//! - 使用 MinGW 自带的 `windres` 编译 `assets/anm.rc`（GNU 路线已安装
//!   gcc-mingw-w64-x86-64，无需额外依赖 crate）；
//! - 产物 `.res` 作为链接参数追加给二进制（`cargo:rustc-link-arg-bins`）。

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // 仅 GNU 工具链的 Windows 目标走 windres；MSVC 原生构建暂不嵌图标
    if target_os != "windows" || target_env != "gnu" {
        return;
    }

    let rc = std::path::Path::new("assets/anm.rc");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR 应由 cargo 提供");
    let out_res = std::path::Path::new(&out).join("anm.res");

    // windres 把 .rc（含 anm.ico 引用）编译为 COFF 资源对象
    let status = std::process::Command::new("x86_64-w64-mingw32-windres")
        .arg(rc)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&out_res)
        .status()
        .expect("无法启动 windres（请确认已安装 gcc-mingw-w64-x86-64）");
    assert!(status.success(), "windres 编译图标资源失败");

    // 把资源对象链接进 anm-win-tray.exe（.rsrc 节）
    println!("cargo:rustc-link-arg-bins={}", out_res.display());
    println!("cargo:rerun-if-changed=assets/anm.rc");
    println!("cargo:rerun-if-changed=assets/anm.ico");

    // GUI 子系统：托盘程序不需要控制台窗口（双击不再闪黑框；
    // 调试输出会丢失，必要时可临时去掉本行改回 console）
    println!("cargo:rustc-link-arg-bins=-Wl,--subsystem,windows");
}
