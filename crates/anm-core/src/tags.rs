//! 标签系统：标签行解析、标签提取、标签行置顶整理、新增标签。
//!
//! ## 规则（readme §11）
//! - **文本标签行**：整行仅含一个或多个 `@xxx`（允许空白分隔）的行；每个 `@xxx`
//!   是一个标签，作用域为整篇笔记；
//! - **段落标签**：段落开头（行首，允许前导空白）出现一个或多个 `@xxx`，其后（同一行）
//!   还有正文 → 这些 `@xxx` 是该段落的段落标签，**仅可出现在段落开头**；
//!   分隔空白兼容全角空格（U+3000，属 Unicode 空白）；
//! - 其他位置出现的 `@xxx`（正文行内、frontmatter 等）不作为标签声明。
//!
//! ## AI 对已有标签的操作边界（readme §6 / §11）
//! - 自主状态下，对已有标签唯一允许的操作是「将标签行移动到文档开头」——
//!   纯位置整理，不合并、不排序、不去重、不改写标签行内容；
//! - 修改、删除已有标签仅在作者显式指令下执行，本模块不提供自主调用原语。

/// 判断一行是否为「文本标签行」（整行仅含标签）。
///
/// 整行（允许首尾空白）由一个或多个 `@xxx` 组成，每段以空白分隔。
pub fn is_tag_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || !t.starts_with('@') {
        return false;
    }
    t.split_whitespace().all(is_tag_token)
}

/// 判断一个空白分隔的 token 是否为标签 token（`@xxx`，`@` 后至少一个字符）
pub fn is_tag_token(token: &str) -> bool {
    token.starts_with('@') && token.len() > 1
}

/// 解析一行标签行，返回标签名列表（不含 `@`，保持出现顺序）
pub fn parse_tag_line(line: &str) -> Vec<String> {
    line.trim()
        .split_whitespace()
        .filter(|t| is_tag_token(t))
        .map(|t| t[1..].to_string())
        .collect()
}

/// 提取一行的**段落标签**：行首（允许前导空白）连续出现一个或多个 `@xxx`，
/// 且其后（同一行）还有非标签正文 → 返回这些标签；否则返回空。
///
/// 规则：
/// - 整行只有标签（无正文）时返回空——那是文本标签行，由 [`is_tag_line`] 处理；
/// - 正文里出现的 `@xxx`（不在行首）不算段落标签；
/// - 分隔空白用 Unicode 空白切分（含全角空格 U+3000）。
pub fn leading_paragraph_tags(line: &str) -> Vec<String> {
    let t = line.trim_start_matches(|c: char| c.is_whitespace());
    if !t.starts_with('@') {
        return Vec::new();
    }
    let mut tags = Vec::new();
    let mut rest = t;
    loop {
        let s = rest.trim_start_matches(|c: char| c.is_whitespace());
        if !s.starts_with('@') {
            break;
        }
        let end = s.find(|c: char| c.is_whitespace()).unwrap_or(s.len());
        let token = &s[..end];
        if !is_tag_token(token) {
            break;
        }
        tags.push(token[1..].to_string());
        rest = &s[end..];
    }
    if tags.is_empty() {
        return Vec::new();
    }
    // 标签序列之后没有正文（只剩空白/空）→ 是文本标签行，不返回
    if rest.trim().is_empty() {
        return Vec::new();
    }
    tags
}

/// 提取文档中的全部标签：文本标签行 ∪ 段落标签（去重，保持首次出现顺序）
pub fn extract_tags(content: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for line in content.lines() {
        if is_tag_line(line) {
            for tag in parse_tag_line(line) {
                if !seen.contains(&tag) {
                    seen.push(tag);
                }
            }
        }
        for tag in leading_paragraph_tags(line) {
            if !seen.contains(&tag) {
                seen.push(tag);
            }
        }
    }
    seen
}

/// 读取文件并提取其标签
pub fn extract_tags_from_file(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    Ok(extract_tags(&content))
}

/// 将文档中已识别的标签行移动到文档开头（纯位置整理）。
///
/// 规则：
/// - 收集文档中所有标签行，保持各自内容与相对顺序不变；
/// - 从原位置移除，统一放到文档开头；
/// - 不合并、不排序、不去重、不改写标签行内容（不改变任何语义信息）；
/// - 标签区与正文之间以一个空行分隔；
/// - 文档中没有标签行时，文档保持不变（除行尾换行归一外不改动）。
pub fn move_tag_lines_to_top(content: &str) -> String {
    let mut tag_lines: Vec<&str> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    for line in content.lines() {
        if is_tag_line(line) {
            tag_lines.push(line);
        } else {
            body.push(line);
        }
    }
    // 去掉 body 顶部的空行（原标签区与正文之间的分隔）
    while let Some(first) = body.first() {
        if first.trim().is_empty() {
            body.remove(0);
        } else {
            break;
        }
    }

    let mut out = String::new();
    for l in &tag_lines {
        out.push_str(l);
        out.push('\n');
    }
    if !tag_lines.is_empty() {
        out.push('\n');
    }
    for l in &body {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// 将文件标签行移动到文档开头（有变化才写盘）
pub fn move_tag_lines_to_top_file(path: &std::path::Path) -> anyhow::Result<bool> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    let updated = move_tag_lines_to_top(&content);
    if updated == content {
        return Ok(false);
    }
    std::fs::write(path, updated)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", path.display()))?;
    Ok(true)
}

/// 文档开头的连续标签行数（0 = 文档开头没有标签区）
fn leading_tag_line_count(content: &str) -> usize {
    content.lines().take_while(|l| is_tag_line(l)).count()
}

/// 向文件新增标签：仅在文档开头的标签区追加不存在的标签行。
///
/// 规则（readme §11）：
/// - 只做「新增」：已存在的标签不重复添加，新标签各自成为一行 `@tag`；
/// - 不触碰、不重排、不改写任何已有标签行（置顶整理是另一个独立原语
///   [`move_tag_lines_to_top_file`] 的职责）；
/// - 文档开头没有标签区时，新建标签区（标签行 + 空行分隔 + 原正文）。
pub fn add_tags(path: &std::path::Path, new_tags: &[String]) -> anyhow::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    let mut existing = extract_tags(&content);
    let mut added = Vec::new();
    for t in new_tags {
        if !existing.contains(t) {
            existing.push(t.clone());
            added.push(t.clone());
        }
    }
    if added.is_empty() {
        return Ok(added);
    }
    let out = if leading_tag_line_count(&content) > 0 {
        insert_tag_lines(&content, &added)
    } else {
        prepend_tag_lines(&content, &added)
    };
    std::fs::write(path, out)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", path.display()))?;
    Ok(added)
}

/// 在文档开头的标签区末尾（最后一个标签行之后）插入新标签行，各占一行。
fn insert_tag_lines(content: &str, new_tags: &[String]) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let n = leading_tag_line_count(content);
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        out.push('\n');
        if i + 1 == n {
            for t in new_tags {
                out.push('@');
                out.push_str(t);
                out.push('\n');
            }
        }
    }
    out
}

/// 文档开头没有标签区时：新建标签区（新标签行 + 空行），正文去掉原有开头空行。
fn prepend_tag_lines(content: &str, new_tags: &[String]) -> String {
    let mut out = String::new();
    for t in new_tags {
        out.push('@');
        out.push_str(t);
        out.push('\n');
    }
    out.push('\n');
    for l in content.lines().skip_while(|l| l.trim().is_empty()) {
        out.push_str(l);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_tag_line() {
        assert!(is_tag_line("@ai"));
        assert!(is_tag_line("@ai @翻译"));
        assert!(is_tag_line("  @ai\t@翻译  "));
        assert!(!is_tag_line(""));
        assert!(!is_tag_line("@"));
        assert!(!is_tag_line("# 标题"));
        assert!(!is_tag_line("正文 @ai 在行内"));
        assert!(!is_tag_line("tags: [ai]"));
    }

    #[test]
    fn parses_tag_line() {
        assert_eq!(parse_tag_line("@ai @翻译"), vec!["ai", "翻译"]);
        assert_eq!(parse_tag_line("@ai/翻译"), vec!["ai/翻译"]);
    }

    #[test]
    fn extracts_tags_from_doc() {
        let doc = "@ai @翻译\n\n# 标题\n正文 @not_tag\n";
        assert_eq!(extract_tags(doc), vec!["ai", "翻译"]);
    }

    #[test]
    fn detects_paragraph_tags() {
        assert_eq!(leading_paragraph_tags("@urgent 记得交报告"), vec!["urgent"]);
        assert_eq!(leading_paragraph_tags("  @a @b 内容"), vec!["a", "b"]);
        // 全角空格分隔（U+3000）
        assert_eq!(leading_paragraph_tags("@a　内容"), vec!["a"]);
        // 整行纯标签 → 是文本标签行，不算段落标签
        assert_eq!(leading_paragraph_tags("@a"), Vec::<String>::new());
        assert_eq!(leading_paragraph_tags("@a @b"), Vec::<String>::new());
        // 行中/行尾的 @xxx 不算段落标签
        assert_eq!(leading_paragraph_tags("正文 @a"), Vec::<String>::new());
        assert_eq!(leading_paragraph_tags("感谢 @张三 的建议"), Vec::<String>::new());
        // 空行/无 @ 开头
        assert_eq!(leading_paragraph_tags(""), Vec::<String>::new());
        assert_eq!(leading_paragraph_tags("# 标题"), Vec::<String>::new());
    }

    #[test]
    fn extract_merges_text_and_paragraph_tags() {
        let doc = "@ai @翻译\n\n@urgent 记得检查备份\n\n正文 @not_tag\n";
        assert_eq!(extract_tags(doc), vec!["ai", "翻译", "urgent"]);
    }

    #[test]
    fn moves_tag_lines_to_top() {
        let doc = "# 标题\n\n正文 @inline\n\n@ai\n更多\n";
        let out = move_tag_lines_to_top(doc);
        assert!(out.starts_with("@ai\n\n# 标题"));
        // 正文行内的 @inline 不是标签行，保留
        assert!(out.contains("正文 @inline"));
        // 原位置的标签行被移动到开头
        assert!(!out.contains("\n@ai\n"));
        assert_eq!(out.matches("@ai").count(), 1);
    }

    #[test]
    fn move_preserves_line_content_and_order() {
        // 不合并、不排序：标签行各自保留，相对顺序不变
        let doc = "# 标题\n\n@b\n\n@ai @x\n更多\n";
        let out = move_tag_lines_to_top(doc);
        assert!(out.starts_with("@b\n@ai @x\n\n# 标题"));
    }

    #[test]
    fn move_keeps_duplicates_unsorted() {
        // 纯位置整理：不去重、不排序
        let doc = "@b\n@a\n@b\n\n正文\n";
        let out = move_tag_lines_to_top(doc);
        assert_eq!(out, "@b\n@a\n@b\n\n正文\n");
    }

    #[test]
    fn move_is_noop_without_tag_lines() {
        let doc = "# 标题\n正文\n";
        let out = move_tag_lines_to_top(doc);
        assert_eq!(out, "# 标题\n正文\n");
    }

    #[test]
    fn move_keeps_document_when_tags_already_at_top() {
        let doc = "@a @b\n\n# 标题\n";
        let out = move_tag_lines_to_top(doc);
        assert_eq!(out, "@a @b\n\n# 标题\n");
    }

    #[test]
    fn add_tags_appends_new_lines_after_existing_header() {
        // 已有头部标签区：新标签追加为新的标签行，不重排已有行
        let dir = std::env::temp_dir().join(format!("anm-tags-add-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("n.md");
        std::fs::write(&path, "@a @b\n\n正文\n").unwrap();
        let added = add_tags(&path, &["c".to_string(), "a".to_string()]).unwrap();
        assert_eq!(added, vec!["c"]);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("@a @b\n@c\n\n正文"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_tags_creates_header_when_none() {
        let dir = std::env::temp_dir().join(format!("anm-tags-add2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("n.md");
        std::fs::write(&path, "# 标题\n正文\n").unwrap();
        let added = add_tags(&path, &["x".to_string()]).unwrap();
        assert_eq!(added, vec!["x"]);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "@x\n\n# 标题\n正文\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn add_tags_noop_when_all_exist() {
        let dir = std::env::temp_dir().join(format!("anm-tags-add3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("n.md");
        let original = "@a\n\n正文\n";
        std::fs::write(&path, original).unwrap();
        let added = add_tags(&path, &["a".to_string()]).unwrap();
        assert!(added.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
