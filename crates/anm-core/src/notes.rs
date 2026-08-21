//! 笔记生命周期：新建笔记（内容编辑交给 $EDITOR，anm 只负责创建与元数据）。

use std::path::Path;

use anyhow::{bail, Context, Result};

/// 在笔记系统根目录内指定目录新建一篇笔记。
///
/// - `dir`：相对根目录的目录路径（必须已存在），会经路径白名单校验防越界；
/// - `title`：用作文件名（清洗掉路径分隔符与控制字符后 + `.md`）；
/// - `content`：可选正文；为空时生成默认标题行；
/// - 目标文件已存在时报错，绝不覆盖；
/// - 返回创建的绝对路径。
pub fn create_note(root: &Path, dir: &str, title: &str, content: &str) -> Result<std::path::PathBuf> {
    let dir_c = crate::path::resolve_dir_in_root(root, dir)?;
    let stem = sanitize_title(title)?;
    let target = dir_c.join(format!("{stem}.md"));
    if target.exists() {
        bail!("文件已存在，未创建（不覆盖）: {}", target.display());
    }
    let body = if content.trim().is_empty() {
        format!("# {title}\n")
    } else {
        content.to_string()
    };
    std::fs::write(&target, body)
        .with_context(|| format!("创建 {} 失败", target.display()))?;
    Ok(target)
}

/// 清洗标题为安全的文件名主干：去除路径分隔符、控制字符，去掉首尾空白与开头的点。
/// 清洗后为空则报错。
fn sanitize_title(title: &str) -> Result<String> {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        bail!("标题无效（清洗后为空）: {title:?}");
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("anm-notes-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("idea")).unwrap();
        dir
    }

    #[test]
    fn creates_note_with_default_header() {
        let root = make_root("create");
        let p = create_note(&root, "idea", "记录灵感", "").unwrap();
        assert!(p.ends_with("idea/记录灵感.md"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# 记录灵感\n");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn creates_note_with_content() {
        let root = make_root("content");
        let p = create_note(&root, "idea", "deep", "正文第一行\n正文第二行\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "正文第一行\n正文第二行\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn refuses_overwrite() {
        let root = make_root("overwrite");
        let p = create_note(&root, "idea", "dup", "").unwrap();
        assert!(p.exists());
        assert!(create_note(&root, "idea", "dup", "").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn sanitizes_dangerous_title() {
        let root = make_root("sanitize");
        // 路径分隔符被清洗（'/'→'-'，开头的点被去除），无法借标题逃逸目录
        let p = create_note(&root, "idea", "../escape", "").unwrap();
        assert!(p.ends_with("idea/-escape.md"));
        // 空标题报错
        assert!(create_note(&root, "idea", "   ", "").is_err());
        assert!(create_note(&root, "idea", "..", "").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn refuses_dir_outside_root() {
        let root = make_root("outside");
        assert!(create_note(&root, "../../tmp", "x", "").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
