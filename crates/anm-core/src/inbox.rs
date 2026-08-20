//! inbox：向默认 skatch.md 追加内容（anw 与 CLI 共用的入闸缓冲）。

use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// 向 skatch.md 追加一条列表项 `- {text}`。
///
/// - text 为空时不写入；
/// - text 中含换行时，逐行以 `- ` 前缀追加；
/// - 文件不存在时创建；父目录不存在时创建。
pub fn append(skatch: &Path, text: &str) -> Result<()> {
    use std::io::Write;

    let text = text.trim();
    if text.is_empty() {
        return Err(anyhow!("内容为空，未写入"));
    }
    if let Some(parent) = skatch.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
    }

    let mut out = String::new();
    let existing = if skatch.exists() {
        Some(
            std::fs::read_to_string(skatch)
                .with_context(|| format!("读取 {} 失败", skatch.display()))?,
        )
    } else {
        None
    };

    if let Some(ex) = &existing {
        // 追加前留一个空行分隔
        if !ex.is_empty() && !ex.ends_with("\n\n") {
            out.push('\n');
        }
    }
    for line in text.lines() {
        out.push_str("- ");
        out.push_str(line);
        out.push('\n');
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(skatch)
        .with_context(|| format!("打开 {} 失败", skatch.display()))?;
    file.write_all(out.as_bytes())
        .with_context(|| format!("写入 {} 失败", skatch.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_list_items() {
        let path = std::env::temp_dir().join(format!("anm-inbox-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        append(&path, "第一条").unwrap();
        append(&path, "第二条").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("- 第一条"));
        assert!(content.contains("- 第二条"));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_empty() {
        let path = std::env::temp_dir().join(format!("anm-inbox-empty-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(append(&path, "  ").is_err());
        std::fs::remove_file(&path).unwrap_or(());
    }
}
