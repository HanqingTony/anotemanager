//! 目录树：枚举笔记系统的一级目录（供 shell 补全与 TUI）。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

/// 一级目录条目
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
}

/// 枚举笔记系统的一级目录。
///
/// - 只列目录，不列文件；
/// - 跳过隐藏目录（以 `.` 开头，如 `.git`、`.sensitive`）；
/// - 按名称排序。
pub fn list_top_dirs(root: &Path) -> Result<Vec<DirEntry>> {
    let mut out = Vec::new();
    let rd = std::fs::read_dir(root)
        .map_err(|e| anyhow!("读取目录 {} 失败: {e}", root.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| anyhow!("读取目录项失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if entry.path().is_dir() {
            out.push(DirEntry {
                name,
                path: entry.path(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// 递归列出笔记系统中的所有目录（含子目录），跳过隐藏目录。
pub fn list_all_dirs(root: &Path) -> Result<Vec<DirEntry>> {
    fn walk(dir: &Path, out: &mut Vec<DirEntry>) -> Result<()> {
        let rd = std::fs::read_dir(dir)
            .map_err(|e| anyhow!("读取目录 {} 失败: {e}", dir.display()))?;
        for entry in rd {
            let entry = entry.map_err(|e| anyhow!("读取目录项失败: {e}"))?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                out.push(DirEntry {
                    name,
                    path: entry.path(),
                });
                walk(&entry.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_dirs_skips_hidden() {
        let dir = std::env::temp_dir().join(format!("anm-tree-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("file.md"), "").unwrap();

        let dirs = list_top_dirs(&dir).unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].name, "a");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
