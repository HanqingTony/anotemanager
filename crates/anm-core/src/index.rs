//! 索引：笔记元数据（路径 / 标题 / 标签 / mtime）的落盘存储。
//!
//! 格式：JSONL，每行一条 [`IndexEntry`]。索引文件位于 `~/.anm/index.jsonl`。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{query, tags};

/// 一条索引记录
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexEntry {
    /// 笔记绝对路径
    pub path: PathBuf,
    /// 标题（文件名去扩展名）
    pub title: String,
    /// 标签
    pub tags: Vec<String>,
    /// 最后修改时间（Unix 秒）
    pub mtime: u64,
}

impl IndexEntry {
    /// 从文件构建索引条目
    pub fn from_file(path: &Path) -> IndexEntry {
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let tags = tags::extract_tags_from_file(path).unwrap_or_default();
        let mtime = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        IndexEntry {
            path: path.to_path_buf(),
            title,
            tags,
            mtime,
        }
    }
}

/// 全量扫描笔记系统，构建索引
pub fn build_index(root: &Path) -> Result<Vec<IndexEntry>> {
    let mut out = Vec::new();
    for path in query::scan_notes(root)? {
        out.push(IndexEntry::from_file(&path));
    }
    Ok(out)
}

/// 保存索引到 JSONL 文件
pub fn save_index(path: &Path, entries: &[IndexEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let mut content = String::new();
    for e in entries {
        content.push_str(&serde_json::to_string(e)?);
        content.push('\n');
    }
    std::fs::write(path, content)
        .with_context(|| format!("写入索引失败: {}", path.display()))
}

/// 加载索引；文件不存在时返回空
pub fn load_index(path: &Path) -> Result<Vec<IndexEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取索引失败: {}", path.display()))?;
    let mut out = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<IndexEntry>(line) {
            Ok(e) => out.push(e),
            Err(e) => eprintln!("索引行解析失败（跳过）: {e}"),
        }
    }
    Ok(out)
}

/// 按标签从索引中查找
pub fn find_by_tag(entries: &[IndexEntry], tags: &[String]) -> Vec<IndexEntry> {
    entries
        .iter()
        .filter(|e| tags.iter().any(|t| e.tags.iter().any(|et| et == t)))
        .cloned()
        .collect()
}

/// 按标题关键字从索引中查找（大小写不敏感）
pub fn find_by_title(entries: &[IndexEntry], keyword: &str) -> Vec<IndexEntry> {
    let kw = keyword.to_lowercase();
    entries
        .iter()
        .filter(|e| e.title.to_lowercase().contains(&kw))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anm-index-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ideas")).unwrap();
        std::fs::write(dir.join("ideas/a.md"), "@ai @翻译\n\n# A\n").unwrap();
        std::fs::write(dir.join("b.md"), "@rust\n\n# B\n").unwrap();
        dir
    }

    #[test]
    fn builds_and_roundtrips_index() {
        let root = make_root("rt");
        let idx = build_index(&root).unwrap();
        assert_eq!(idx.len(), 2);

        let file = std::env::temp_dir().join(format!("anm-idx-rt-{}", std::process::id()));
        save_index(&file, &idx).unwrap();
        let loaded = load_index(&file).unwrap();
        assert_eq!(idx, loaded);

        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn finds_from_index() {
        let root = make_root("find");
        let idx = build_index(&root).unwrap();
        let hits = find_by_tag(&idx, &["ai".to_string()]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "a");

        let by_title = find_by_title(&idx, "B");
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].title, "b");

        std::fs::remove_dir_all(&root).unwrap();
    }
}
