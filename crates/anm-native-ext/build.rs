//! 嵌入托盘图标资源（anm.ico → id 1）。
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let ico = std::path::Path::new("assets/anm.ico");
    if ico.exists() {
        let rc = format!(
            "1 ICON \"{}\"",
            std::fs::canonicalize(ico).unwrap().display()
        );
        let rc_path = std::path::Path::new(&out).join("anm.rc");
        std::fs::write(&rc_path, rc).unwrap();
        println!("cargo:rerun-if-changed=assets/anm.ico");
        let mut cmd = std::process::Command::new("x86_64-w64-mingw32-windres");
        cmd.arg(&rc_path).arg("-O").arg("coff");
        let obj = std::path::Path::new(&out).join("anm_icon.o");
        cmd.arg("-o").arg(&obj);
        if cmd.status().map(|s| s.success()).unwrap_or(false) {
            println!("cargo:rustc-link-arg={}", obj.display());
        }
    }
}
