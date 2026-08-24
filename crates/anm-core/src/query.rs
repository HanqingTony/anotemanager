//! 笔记查询：扫描笔记系统并按标签 / 标题检索。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::tags;

/// 一条笔记的元信息
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// skatch（inbox）文件的段落列表：**按行分段**——`\r` / `\n` 都是分隔符，
/// 每行（去除首尾空白后非空）即一个段落。
///
/// 不做任何格式识别（短行 / `##` 标题行都与普通行同等对待）；读取时先把
/// `\r\n` 规范化为 `\n`（内置编辑器基于 RichEdit 保存时可能写入 CRLF）。
pub fn skatch_segments(path: &std::path::Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(path)?;
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    Ok(text
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

/// 判断路径是否指向一篇笔记文件（扩展名在 `NOTE_EXTENSIONS` 内且非隐藏文件）。
/// 供 MCP / daemon 等入口在路径白名单之上做「只读笔记」的二次校验。
pub fn is_note_path(path: &Path) -> bool {
    is_note_file(path)
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

/// 全文检索命中
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ContentHit {
    /// 绝对路径
    pub file: PathBuf,
    /// 命中上下文片段
    pub snippet: String,
    /// 命中次数（排序依据）
    pub score: usize,
}

/// 全文搜索：对笔记正文做大小写不敏感的子串匹配。
///
/// 返回按命中次数降序排列、截断到 `limit` 条的结果；
/// 每条附一段包含首个命中的上下文片段（供 agent 判断相关性，避免整库灌入上下文）。
pub fn search_content(root: &Path, keyword: &str, limit: usize) -> Result<Vec<ContentHit>> {
    let kw = keyword.trim().to_lowercase();
    if kw.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    for path in scan_notes(root)? {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let lower = content.to_lowercase();
        let score = lower.matches(&kw).count();
        if score == 0 {
            continue;
        }
        hits.push(ContentHit {
            file: path,
            snippet: make_snippet(&content, &lower, &kw),
            score,
        });
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score));
    hits.truncate(limit);
    Ok(hits)
}

/// 生成包含首个命中的上下文片段（每侧约 40 字符，截断处加省略号）
fn make_snippet(content: &str, lower: &str, kw: &str) -> String {
    const HALF: usize = 40;
    let start = match lower.find(kw) {
        Some(i) => i,
        None => return content.chars().take(HALF * 2).collect(),
    };
    let (before, _) = content.split_at(start);
    let before_count = before.chars().count();
    let lead = before_count.saturating_sub(HALF);
    let s: String = content.chars().skip(lead).take(HALF * 2 + kw.len()).collect();
    let mut out = String::new();
    if lead > 0 {
        out.push('…');
    }
    out.push_str(&s);
    if content.chars().count() > lead + HALF * 2 + kw.len() {
        out.push('…');
    }
    out
}

/// 列出某目录下直接包含的笔记文件（非递归），按文件名排序。
pub fn list_in_dir(root: &Path, rel_dir: &str) -> Result<Vec<NoteInfo>> {
    let dir = crate::path::resolve_dir_in_root(root, rel_dir)?;
    let mut out = Vec::new();
    let rd = std::fs::read_dir(&dir)
        .map_err(|e| anyhow!("读取目录 {} 失败: {e}", dir.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| anyhow!("读取目录项失败: {e}"))?;
        let p = entry.path();
        if !is_note_file(&p) {
            continue;
        }
        let title = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let tags = match std::fs::read_to_string(&p) {
            Ok(content) => tags::extract_tags(&content),
            Err(_) => Vec::new(),
        };
        out.push(NoteInfo { path: p, title, tags });
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(out)
}

/// 一级目录及其直接笔记、直接子目录的总览条目（readme §12 聚合原语：
/// 只拼数据，不组织语言）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DirOverview {
    /// 目录名
    pub name: String,
    /// 目录绝对路径
    pub path: PathBuf,
    /// 目录下直接包含的笔记（非递归，按标题排序）
    pub notes: Vec<NoteInfo>,
    /// 直接子目录名（非递归、跳过隐藏；供托盘临时子卡片使用）
    pub subdirs: Vec<String>,
}

/// 聚合查询：列出所有一级目录，并附带每个目录下直接包含的笔记与子目录。
///
/// 供需要"一次取全"的客户端使用（如 anm-tray-win 覆盖层的卡片总览）；
/// 内部复用 `list_top_dirs` + `list_in_dir` 两个确定性原语。
pub fn overview(root: &Path) -> Result<Vec<DirOverview>> {
    let mut out = Vec::new();
    for dir in crate::tree::list_top_dirs(root)? {
        out.push(overview_of_dir(&dir.path, &dir.name, root, &dir.name)?);
    }
    Ok(out)
}

/// 查询任意（根目录内的）目录的总览：其直接笔记 + 直接子目录。
///
/// 供托盘"临时子卡片"使用：点击卡片中的子目录行时，拉取该子目录的内容。
pub fn overview_dir(root: &Path, rel_dir: &str) -> Result<DirOverview> {
    let dir = crate::path::resolve_dir_in_root(root, rel_dir)?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    overview_of_dir(&dir, &name, root, rel_dir)
}

/// 从已解析的目录路径构建总览条目（内部共用实现）。
fn overview_of_dir(
    dir: &Path,
    name: &str,
    root: &Path,
    rel: &str,
) -> Result<DirOverview> {
    let notes = list_in_dir(root, rel)?;
    let subdirs = crate::tree::list_top_dirs(dir)?
        .into_iter()
        .map(|d| d.name)
        .collect();
    Ok(DirOverview {
        name: name.to_string(),
        path: dir.to_path_buf(),
        notes,
        subdirs,
    })
}

/// 最近修改的笔记条目
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentNote {
    /// 绝对路径
    pub path: PathBuf,
    /// 标题：文件名（去扩展名）
    pub title: String,
    /// 标签
    pub tags: Vec<String>,
    /// 最后修改时间（Unix 秒）
    pub modified: u64,
}

/// 按最后修改时间取最近的 n 条笔记（时间新者在前）。
pub fn recent(root: &Path, n: usize) -> Result<Vec<RecentNote>> {
    let mut notes = Vec::new();
    for path in scan_notes(root)? {
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let tags = match std::fs::read_to_string(&path) {
            Ok(content) => tags::extract_tags(&content),
            Err(_) => Vec::new(),
        };
        notes.push(RecentNote { path, title, tags, modified });
    }
    notes.sort_by(|a, b| b.modified.cmp(&a.modified));
    notes.truncate(n);
    Ok(notes)
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

    #[test]
    fn searches_content() {
        let root = make_root("content");
        std::fs::write(root.join("ideas/note-a.md"), "@ai\n\n# A\n中文正文 contains 关键词\n").unwrap();
        std::fs::write(root.join("ideas/note-b.md"), "@ai\n\n# B\n另一篇无关\n").unwrap();
        let hits = search_content(&root, "关键词", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].file.ends_with("note-a.md"));
        assert!(hits[0].snippet.contains("关键词"));
        assert_eq!(hits[0].score, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn search_content_respects_limit_and_score() {
        let root = make_root("score");
        std::fs::write(root.join("a.md"), "重复 重复 重复 重复 词\n").unwrap();
        std::fs::write(root.join("b.md"), "词\n").unwrap();
        let hits = search_content(&root, "词", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits[0].file.ends_with("a.md")); // score 高者在前
        let limited = search_content(&root, "词", 1).unwrap();
        assert_eq!(limited.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lists_dir_notes_non_recursive() {
        let root = make_root("listdir");
        std::fs::create_dir_all(root.join("mylist/sub")).unwrap();
        std::fs::write(root.join("mylist/note-a.md"), "@ai\n").unwrap();
        std::fs::write(root.join("mylist/note-b.md"), "@ai\n").unwrap();
        std::fs::write(root.join("mylist/sub/deep.md"), "@ai\n").unwrap();
        let notes = list_in_dir(&root, "mylist").unwrap();
        assert_eq!(notes.len(), 2);
        // 拒绝越界目录
        assert!(list_in_dir(&root, "../..").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn overview_groups_dirs_with_their_notes() {
        let root = make_root("overview"); // 夹具自带 ideas/（含 2 条笔记）与 .hidden/
        std::fs::create_dir_all(root.join("ref")).unwrap();
        std::fs::create_dir_all(root.join("ref/sub1")).unwrap();
        std::fs::create_dir_all(root.join("ref/sub2")).unwrap();
        std::fs::write(root.join("ref/linux.md"), "\n").unwrap();
        std::fs::write(root.join("top.md"), "\n").unwrap(); // 根目录文件不属于任何一级目录卡片

        let ov = overview(&root).unwrap();
        assert_eq!(ov.len(), 2); // ideas + ref（.hidden 被跳过）
        assert_eq!(ov[0].name, "ideas"); // 按名称排序在前
        assert_eq!(ov[0].notes.len(), 2);
        assert_eq!(ov[0].subdirs.len(), 0);
        assert_eq!(ov[1].name, "ref");
        assert_eq!(ov[1].notes.len(), 1);
        assert_eq!(ov[1].notes[0].title, "linux");
        assert_eq!(ov[1].subdirs, vec!["sub1", "sub2"]);

        // 任意子目录的总览（临时子卡片数据源）
        let sub = overview_dir(&root, "ref/sub1").unwrap();
        assert_eq!(sub.name, "sub1");
        assert!(sub.path.ends_with("ref/sub1"));
        assert!(sub.notes.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recent_sorts_by_mtime() {
        // 独立根目录（不用夹具），全部文件显式设置 mtime，避免时间粒度与夹具干扰
        let root = std::env::temp_dir()
            .join(format!("anm-query-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for name in ["a.md", "b.md", "c.md"] {
            std::fs::write(root.join(name), format!("# {name}\n")).unwrap();
        }
        for (name, secs) in [("a.md", 100u64), ("b.md", 200), ("c.md", 300)] {
            let f = std::fs::OpenOptions::new().write(true).open(root.join(name)).unwrap();
            f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
            drop(f);
        }
        let notes = recent(&root, 2).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].title, "c");
        assert_eq!(notes[1].title, "b");
        assert!(notes[0].modified >= notes[1].modified);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
