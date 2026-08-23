//! WSL 路径 → Windows 路径转换（托盘"点击打开"前的地址翻译）。
//!
//! 卡片里的笔记路径来自 WSL 内的 anm-core 服务（如 `/mnt/c/Users/hanqi/
//! OneDrive/znote/idea/a.md`），而 ShellExecute 需要 Windows 路径。转换规则：
//! - `/mnt/<盘符>/…` → `<盘符>:\…`（覆盖 znote 挂在 OneDrive 的常见情况）；
//! - 其余 WSL 路径（`/home/…`、`/root/…`）v1 原样返回（无法打开时由
//!   调用方忽略；后续如需支持可引入 `\\wsl.localhost\<发行版>\` 前缀映射）。

/// 把 WSL 绝对路径翻译为 Windows 路径；无法识别的路径原样返回。
pub fn to_windows(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("/mnt/") {
        // /mnt/c/Users/... → C:\Users\...
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b'/' {
            let drive = bytes[0] as char;
            // 跳过 "/mnt/" 与盘符及其后的 '/'，保留原始大小写
            let tail = &path["/mnt/".len() + 2..];
            return format!("{}:\\{}", drive.to_ascii_uppercase(), tail.replace('/', "\\"));
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// /mnt/c 前缀 → C:\ 盘符（保留路径大小写，分隔符反转）。
    #[test]
    fn mnt_c_drive_translation() {
        assert_eq!(
            to_windows("/mnt/c/Users/hanqi/OneDrive/znote/idea/a.md"),
            "C:\\Users\\hanqi\\OneDrive\\znote\\idea\\a.md"
        );
    }

    /// 其他盘符同样处理（/mnt/d → D:\）。
    #[test]
    fn other_drive_letters() {
        assert_eq!(to_windows("/mnt/d/data/notes/b.md"), "D:\\data\\notes\\b.md");
    }

    /// 非 /mnt 路径 v1 原样返回（不做 \\wsl.localhost 映射）。
    #[test]
    fn non_mnt_path_passes_through() {
        assert_eq!(to_windows("/home/tony/znote/x.md"), "/home/tony/znote/x.md");
    }
}
