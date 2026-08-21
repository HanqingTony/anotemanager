//! 路径白名单：把用户 / agent 提供的路径限定在笔记系统根目录内，防目录穿越。
//!
//! 所有接受路径参数的入口（CLI、MCP、daemon）都应经本模块校验后再触碰文件系统。

use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

/// 校验一个「应当已存在」的路径，返回根目录内的规范化绝对路径。
///
/// - 相对路径按笔记系统根目录解析；绝对路径直接校验；
/// - 经 `canonicalize` 解析符号链接与 `..`，再确认结果仍在根目录内
///   （符号链接指向库外时 canonicalize 会解析出真实路径从而被拦下）；
/// - 目标必须是普通文件（目录等非文件路径会被拒绝，配合读取 / 标签类操作）。
pub fn resolve_file_in_root(root: &Path, user_path: &str) -> Result<PathBuf> {
    let root_c = root
        .canonicalize()
        .with_context(|| format!("笔记系统根目录不可访问: {}", root.display()))?;
    let joined = join_against_root(&root_c, Path::new(user_path))?;
    let canon = joined
        .canonicalize()
        .with_context(|| format!("路径不存在或不可访问: {user_path}"))?;
    if !canon.starts_with(&root_c) {
        bail!("路径超出笔记系统范围: {user_path}");
    }
    if !canon.is_file() {
        bail!("不是笔记文件: {user_path}");
    }
    Ok(canon)
}

/// 校验一个「可能还不存在」的路径（新建文件等），返回根目录内的绝对路径。
///
/// 只做词法级校验（`.``..` 归一化后必须仍在根目录内），不做 `canonicalize`，
/// 因为目标文件可能尚未创建；符号链接逃逸由「已存在」入口
/// `resolve_file_in_root` 负责拦截。
pub fn resolve_new_in_root(root: &Path, user_path: &str) -> Result<PathBuf> {
    let root_c = root
        .canonicalize()
        .with_context(|| format!("笔记系统根目录不可访问: {}", root.display()))?;
    let joined = join_against_root(&root_c, Path::new(user_path))?;
    let normalized = normalize(&joined);
    if !normalized.starts_with(&root_c) {
        bail!("路径超出笔记系统范围: {user_path}");
    }
    Ok(normalized)
}

/// 校验一个目录路径（列表、新建的目标目录），必须是根目录内已存在的目录。
pub fn resolve_dir_in_root(root: &Path, user_path: &str) -> Result<PathBuf> {
    let root_c = root
        .canonicalize()
        .with_context(|| format!("笔记系统根目录不可访问: {}", root.display()))?;
    let joined = join_against_root(&root_c, Path::new(user_path))?;
    let canon = joined
        .canonicalize()
        .with_context(|| format!("目录不存在或不可访问: {user_path}"))?;
    if !canon.starts_with(&root_c) {
        bail!("目录超出笔记系统范围: {user_path}");
    }
    if !canon.is_dir() {
        bail!("不是目录: {user_path}");
    }
    Ok(canon)
}

/// 把用户路径与根目录合并：绝对路径原样使用，相对路径拼在根目录下。
/// 空路径直接拒绝；词法逃逸由调用方的 normalize + starts_with 兜底。
fn join_against_root(root: &Path, user: &Path) -> Result<PathBuf> {
    if user.as_os_str().is_empty() {
        bail!("路径不能为空");
    }
    Ok(if user.is_absolute() {
        user.to_path_buf()
    } else {
        root.join(user)
    })
}

/// 词法归一化：消除 `.` 与 `..` 组件（`..` 越过根目录时保留在结果中，
/// 由调用方 starts_with 判定为越界）。
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("anm-path-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ref")).unwrap();
        std::fs::create_dir_all(dir.join("secret")).unwrap();
        std::fs::write(dir.join("ref/linux.md"), "# linux\n").unwrap();
        std::fs::write(dir.join("secret/creds.md"), "# secret\n").unwrap();
        dir
    }

    #[test]
    fn resolves_relative_inside_root() {
        let root = make_root("rel");
        let p = resolve_file_in_root(&root, "ref/linux.md").unwrap();
        assert!(p.ends_with("ref/linux.md"));
        assert!(p.starts_with(root.canonicalize().unwrap()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolves_absolute_inside_root() {
        let root = make_root("abs");
        let abs = root.join("ref/linux.md");
        let p = resolve_file_in_root(&root, &abs.to_string_lossy()).unwrap();
        assert_eq!(p, abs.canonicalize().unwrap());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_traversal() {
        let root = make_root("traverse");
        // 相对 `..` 逃逸
        assert!(resolve_file_in_root(&root, "../etc/passwd").is_err());
        assert!(resolve_file_in_root(&root, "ref/../../etc/passwd").is_err());
        assert!(resolve_new_in_root(&root, "../../x.md").is_err());
        // 绝对路径逃逸
        assert!(resolve_file_in_root(&root, "/etc/passwd").is_err());
        assert!(resolve_new_in_root(&root, "/etc/passwd").is_err());
        // 空路径
        assert!(resolve_file_in_root(&root, "").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn accepts_benign_parent_dir() {
        let root = make_root("benign");
        // /root/a/.. 仍在根目录内，不应误杀
        let p = resolve_new_in_root(&root, "ref/../idea.md").unwrap();
        assert_eq!(p, root.canonicalize().unwrap().join("idea.md"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rejects_non_file() {
        let root = make_root("nonfile");
        assert!(resolve_file_in_root(&root, "ref").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolves_dir() {
        let root = make_root("dir");
        let d = resolve_dir_in_root(&root, "ref").unwrap();
        assert!(d.ends_with("ref"));
        assert!(resolve_dir_in_root(&root, "/etc").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolves_new_path() {
        let root = make_root("new");
        let p = resolve_new_in_root(&root, "idea/新建.md").unwrap();
        assert!(p.ends_with("idea/新建.md"));
        assert!(p.starts_with(root.canonicalize().unwrap()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
