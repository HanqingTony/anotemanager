//! 笔记查询：扫描笔记系统并按标签 / 标题检索。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::tags;

/// 一条笔记的元信息
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NoteInfo {
    /// 绝对路径
    pub path: PathBuf,
    /// 标题：文件名（去扩展名）
    pub title: String,
    /// 标签（来自头部标签区与文档标签行）
    pub tags: Vec<String>,
}

/// 视为笔记的扩展名
pub const NOTE_EXTENSIONS: &[&str] = &["md", "markdown", "txt"];

fn is_note_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with('.') {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| NOTE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 递归扫描笔记系统中的所有笔记文件（跳过隐藏目录）
pub fn scan_notes(root: &Path) -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        let rd = std::fs::read_dir(dir)
            .map_err(|e| anyhow!("读取目录 {} 失败: {e}", dir.display()))?;
        for entry in rd {
            let entry = entry.map_err(|e| anyhow!("读取目录项失败: {e}"))?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out)?;
            } else if is_note_file(&p) {
                out.push(p);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    out.sort();
    Ok(out)
}

/// 收集全部笔记信息（标题 + 标签）
pub fn all_notes(root: &Path) -> Result<Vec<NoteInfo>> {
    let mut out = Vec::new();
    for path in scan_notes(root)? {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let tags = match std::fs::read_to_string(&path) {
            Ok(content) => tags::extract_tags(&content),
            Err(_) => Vec::new(),
        };
        out.push(NoteInfo { path, title, tags });
    }
    Ok(out)
}

/// 按标签查找笔记：匹配任一标签即命中
pub fn find_by_tag(root: &Path, tags: &[String]) -> Result<Vec<NoteInfo>> {
    let notes = all_notes(root)?;
    Ok(notes
        .into_iter()
        .filter(|n| tags.iter().any(|t| n.tags.iter().any(|nt| nt == t)))
        .collect())
}

/// 按标题 / 文件名关键字查找（大小写不敏感，子串匹配）
pub fn find_by_title(root: &Path, keyword: &str) -> Result<Vec<NoteInfo>> {
    let kw = keyword.to_lowercase();
    let notes = all_notes(root)?;
    Ok(notes
        .into_iter()
        .filter(|n| n.title.to_lowercase().contains(&kw))
        .collect())
}

/// 列出系统中出现的所有标签（去重排序）
pub fn all_tags(root: &Path) -> Result<Vec<String>> {
    let mut set: Vec<String> = Vec::new();
    for note in all_notes(root)? {
        for t in note.tags {
            if !set.contains(&t) {
                set.push(t);
            }
        }
    }
    set.sort();
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anm-query-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ideas")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(
            dir.join("ideas/note-a.md"),
            "@ai @翻译\n\n# A\n正文\n",
        )
        .unwrap();
        std::fs::write(dir.join("ideas/note-b.txt"), "@ai\n\n# B\n").unwrap();
        std::fs::write(dir.join("plain.md"), "# 无标签\n").unwrap();
        std::fs::write(dir.join(".hidden/secret.md"), "@secret\n\n# S\n").unwrap();
        dir
    }

    #[test]
    fn scans_only_notes() {
        let root = make_root("scan");
        let files = scan_notes(&root).unwrap();
        assert_eq!(files.len(), 3);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn finds_by_tag() {
        let root = make_root("find");
        let hits = find_by_tag(&root, &["ai".to_string()]).unwrap();
        assert_eq!(hits.len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn collects_all_tags() {
        let root = make_root("tags");
        let tags = all_tags(&root).unwrap();
        assert_eq!(tags, vec!["ai", "翻译"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
