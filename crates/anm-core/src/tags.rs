//! 标签系统：标签行解析、标签提取、头部标签区同步。
//!
//! ## 规则
//! - 标签行：**整行仅含一个或多个 `@xxx`**（允许空白分隔）的行。
//! - 每个 `@xxx` 识别为一个标签（标签名不含 `@`）。
//! - 其他位置出现的 `@xxx`（正文行内、frontmatter 等）不作为标签声明。
//! - 文档内的标签永远是扁平的；层级仅存在于 anm 内部数据模型。
//!
//! ## 头部标签区
//! anm 自动将标签维护至文档头部：重写文件顶部为标签行，
//! 移除正文中的标签行，头部标签区与正文之间以空行分隔。

/// 判断一行是否为「仅含标签」的行。
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

/// 提取文档中的全部标签（所有标签行的并集，去重，保持首次出现顺序）
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
    }
    seen
}

/// 读取文件并提取其标签
pub fn extract_tags_from_file(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    Ok(extract_tags(&content))
}

/// 将文档的标签统一维护到头部标签区。
///
/// 规则：
/// - 收集文档中所有标签行的标签，去重并排序；
/// - 无标签时移除头部标签区；
/// - 有标签时重写文件顶部为一行标签行 `@a @b @c`，与正文之间空一行；
/// - 移除正文中所有标签行，其余内容（含空行）保持不变。
pub fn sync_header(content: &str) -> String {
    let mut tags = extract_tags(content);
    tags.sort();
    tags.dedup();

    // 过滤掉所有标签行
    let mut body: Vec<&str> = content.lines().filter(|l| !is_tag_line(l)).collect();
    // 去掉 body 顶部的空行（原头部区与正文之间的分隔）
    while let Some(first) = body.first() {
        if first.trim().is_empty() {
            body.remove(0);
        } else {
            break;
        }
    }

    let mut out = String::new();
    if !tags.is_empty() {
        out.push_str(&format!("@{}", tags.join(" @")));
        out.push('\n');
        out.push('\n');
    }
    for l in &body {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// 将文件标签同步至头部标签区（有变化才写盘）
pub fn sync_header_file(path: &std::path::Path) -> anyhow::Result<bool> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    let updated = sync_header(&content);
    if updated == content {
        return Ok(false);
    }
    std::fs::write(path, updated)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", path.display()))?;
    Ok(true)
}

/// 向文件添加标签：在头部标签区加入指定标签后同步。
/// 若标签已存在则不重复添加。
pub fn add_tags(path: &std::path::Path, new_tags: &[String]) -> anyhow::Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("读取 {} 失败: {e}", path.display()))?;
    let mut tags = extract_tags(&content);
    let mut added = Vec::new();
    for t in new_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
            added.push(t.clone());
        }
    }
    if added.is_empty() {
        return Ok(added);
    }
    // 构造新的头部行 + 原有正文
    tags.sort();
    tags.dedup();
    let mut body: Vec<&str> = content.lines().filter(|l| !is_tag_line(l)).collect();
    while let Some(first) = body.first() {
        if first.trim().is_empty() {
            body.remove(0);
        } else {
            break;
        }
    }
    let mut out = String::new();
    out.push_str(&format!("@{}", tags.join(" @")));
    out.push('\n');
    out.push('\n');
    for l in &body {
        out.push_str(l);
        out.push('\n');
    }
    std::fs::write(path, out)
        .map_err(|e| anyhow::anyhow!("写入 {} 失败: {e}", path.display()))?;
    Ok(added)
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
    fn syncs_header() {
        let doc = "# 标题\n\n正文 @inline\n\n@ai\n更多\n";
        let out = sync_header(doc);
        assert!(out.starts_with("@ai\n\n# 标题"));
        // 正文行内的 @inline 不是标签行，保留
        assert!(out.contains("正文 @inline"));
        // 正文中的标签行 @ai 被移除
        assert!(!out.contains("\n@ai\n"));
    }

    #[test]
    fn syncs_header_removes_inline_tag_lines() {
        let doc = "# 标题\n\n@b\n\n@ai\n更多\n";
        let out = sync_header(doc);
        assert!(out.starts_with("@ai @b\n\n# 标题"));
        assert!(!out.contains("\n@b\n"));
    }

    #[test]
    fn syncs_header_sorted_and_dedup() {
        let doc = "@b\n@a\n@b\n\n正文\n";
        let out = sync_header(doc);
        assert!(out.starts_with("@a @b\n\n正文"));
    }

    #[test]
    fn removes_header_when_no_tags() {
        let doc = "# 标题\n正文\n";
        let out = sync_header(doc);
        assert_eq!(out, "# 标题\n正文\n");
    }
}
