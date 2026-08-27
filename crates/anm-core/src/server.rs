//! IPC 服务：anm-core 服务面向三个应用（anm / anw / anm-win-tray）的查询/写入端点。
//!
//! - 传输：TCP + JSON 行（协议类型见 [`anm_core::protocol`]）；
//! - 每行是 [`anm_core::protocol::Envelope`]（令牌 + 请求）：服务端配置
//!   `[server] token` 后校验令牌，不匹配一律拒绝；
//! - 每个连接只处理一个请求：读一行请求 → 分发执行 → 写一行响应；
//! - 执行全部走 lib 的确定性原语（现场扫描），不维护任何持久索引；
//! - 写入类命令只覆盖 readme §6 允许的"新增/低风险追加"范围，路径参数
//!   一律经 `path` 模块白名单校验（防目录穿越 / 符号链接逃逸）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use anm_core::config::Config;
use anm_core::protocol::{Envelope, Request, Response};
use anm_core::{inbox, notes, path, query, tags, tree};

/// 启动 IPC 服务：绑定配置的 `[server]` 地址，accept 循环中为每个连接
/// 派生一个异步任务处理单个请求。本函数阻塞运行直到进程退出。
pub async fn run(cfg: Config) -> Result<()> {
    let addr = format!("{}:{}", cfg.server.host, cfg.server.port);
    let listener = TcpListener::bind(&addr)
        .await
        .with_context(|| format!("绑定 IPC 地址 {addr} 失败（端口被占用？）"))?;
    println!("anm-core: IPC 服务已启动 → {addr}");

    loop {
        let (stream, _peer) = listener.accept().await?;
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &cfg).await {
                eprintln!("anm-core: IPC 连接处理失败: {e:#}");
            }
        });
    }
}

/// 处理一个 IPC 连接：读一行请求 → 分发 → 写一行响应。
///
/// 任何解析 / 执行错误都不会让连接悬挂：统一落成 `ok: false` 的响应行。
async fn handle_connection(stream: TcpStream, cfg: &Config) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    if line.trim().is_empty() {
        return Ok(());
    }

    let env = match serde_json::from_str::<Envelope>(line.trim()) {
        Ok(env) => env,
        Err(e) => return Ok(write_response(&mut writer, Response::err(format!("请求解析失败: {e}"))).await?),
    };
    let resp = match check_token(cfg, env.token.as_deref()) {
        Ok(()) => dispatch(cfg, env.request),
        Err(msg) => Response::err(msg),
    };
    write_response(&mut writer, resp).await?;
    Ok(())
}

/// 把响应写成一行 JSON 并冲刷连接。
async fn write_response(writer: &mut (impl AsyncWriteExt + Unpin), resp: Response) -> Result<()> {
    let out = serde_json::to_string(&resp)?;
    writer.write_all(out.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// 令牌校验：`[server] token` 未配置时放行；配置后要求请求令牌完全一致。
/// 比较使用常数时间（防时序侧信道——未来对公网暴露时同样适用）。
fn check_token(cfg: &Config, presented: Option<&str>) -> Result<(), String> {
    match &cfg.server.token {
        None => Ok(()),
        Some(expected) => {
            let ok = match presented {
                Some(p) => {
                    if p.len() != expected.len() {
                        false
                    } else {
                        p.bytes()
                            .zip(expected.bytes())
                            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                            == 0
                    }
                }
                None => false,
            };
            if ok {
                Ok(())
            } else {
                Err("访问令牌无效（若服务端配置了 [server] token，客户端必须携带相同令牌）".into())
            }
        }
    }
}

/// 分发一个 IPC 请求到对应的 lib 原语，返回响应。
///
/// 不涉及任何语义判断（anm-core 不感知场景概念）；失败时返回
/// `ok: false` 的响应，由客户端展示错误。
fn dispatch(cfg: &Config, req: Request) -> Response {
    match exec(cfg, req) {
        Ok(data) => Response::ok(data),
        Err(e) => Response::err(format!("{e:#}")),
    }
}

/// 执行一个 IPC 请求，返回结果 JSON；所有错误统一为 `anyhow::Error` 向上传播。
fn exec(cfg: &Config, req: Request) -> anyhow::Result<serde_json::Value> {
    match req {
        Request::Dirs => json_ok(tree::list_top_dirs(&cfg.root)),
        Request::Overview => json_ok(query::overview(&cfg.root)),
        Request::OverviewDir { dir } => json_ok(query::overview_dir(&cfg.root, &dir)),
        Request::FindTag { tags } => json_ok(query::find_by_tag(&cfg.root, &tags)),
        Request::Search { keyword } => json_ok(query::find_by_title(&cfg.root, &keyword)),
        Request::Tags => json_ok(query::all_tags(&cfg.root)),
        Request::TagMoveTop { path } => {
            let resolved = resolve_note_arg(cfg, &path)?;
            let changed = tags::move_tag_lines_to_top_file(&resolved)?;
            Ok(serde_json::json!({ "path": resolved, "changed": changed }))
        }
        Request::TagAdd { path, tags } => {
            let resolved = resolve_note_arg(cfg, &path)?;
            let added = tags::add_tags(&resolved, &tags)?;
            Ok(serde_json::json!({ "path": resolved, "added": added }))
        }
        Request::InboxAppend { text } => {
            inbox::append(&cfg.skatch, &text)?;
            Ok(serde_json::json!({ "written": true, "skatch": cfg.skatch }))
        }
        Request::ReadNote { path } => {
            let resolved = resolve_note_arg(cfg, &path)?;
            let content = std::fs::read_to_string(&resolved)?;
            Ok(serde_json::json!({
                "path": resolved,
                "root": cfg.root,
                "content": content,
                "chars": content.chars().count(),
            }))
        }
        Request::WriteNote { path, content } => {
            let resolved = resolve_note_arg(cfg, &path)?;
            // 统一 POSIX 换行：RichEdit 读取的文本是 CRLF，写回前规范化，
            // 避免笔记文件被编辑器保存后混入 \r\n（破坏空行分段）
            let content = content.replace("\r\n", "\n").replace('\r', "\n");
            std::fs::write(&resolved, content)?;
            Ok(serde_json::json!({ "path": resolved, "root": cfg.root, "written": true }))
        }
        Request::CreateNote { dir, title } => {
            // 标题允许带 .md/.markdown/.txt 后缀，剥离后再交给 create_note（内部补 .md）
            let stem = strip_note_ext(&title);
            let created = notes::create_note(&cfg.root, &dir, &stem, "")?;
            Ok(serde_json::json!({ "path": created, "root": cfg.root, "created": true }))
        }
        Request::RenameNote { from, to } => {
            let from_r = resolve_note_arg(cfg, &from)?;
            // 目标可能尚不存在：用词法级解析（canonicalize 要求文件已存在）
            let to_r = path::resolve_new_in_root(&cfg.root, &to)?;
            // 目标尚不存在，只能按扩展名校验（is_note_path 要求文件已存在）
            let ext_ok = to_r
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| query::NOTE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false);
            if !ext_ok {
                bail!("目标不是笔记文件（仅支持 .md/.markdown/.txt）: {to}");
            }
            if from_r.parent() != to_r.parent() {
                bail!("重命名仅限同目录内（不允许移动目录）");
            }
            if from_r == to_r {
                bail!("新旧路径相同");
            }
            if to_r.exists() {
                bail!("目标已存在，不覆盖: {}", to_r.display());
            }
            std::fs::rename(&from_r, &to_r)?;
            Ok(serde_json::json!({ "from": from_r, "to": to_r, "root": cfg.root, "renamed": true }))
        }
        Request::Skatch => {
            let segments = query::skatch_segments(&cfg.skatch)?;
            Ok(serde_json::json!({
                "path": cfg.skatch,
                "root": cfg.root,
                "segments": segments,
            }))
        }
        Request::SkatchExtract { dir, index } => {
            let segments = query::skatch_segments(&cfg.skatch)?;
            let segment = segments
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("段落下标越界（共 {} 段）: {index}", segments.len()))?
                .clone();
            // 从 skatch 中移除该段（其余段落按空行重新拼接写回）
            let rest = segments
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");
            std::fs::write(&cfg.skatch, if rest.is_empty() { String::new() } else { rest + "\n" })?;
            // 段落首行作为新笔记标题
            let title = segment.lines().next().unwrap_or("新笔记").to_string();
            let created = notes::create_note(&cfg.root, &dir, &title, &segment)?;
            Ok(serde_json::json!({ "path": created, "root": cfg.root, "extracted": true }))
        }
        Request::SkatchInsert { from } => {
            let from_r = resolve_note_arg(cfg, &from)?;
            if from_r == cfg.skatch {
                bail!("不能把 skatch 并入自身");
            }
            let content = std::fs::read_to_string(&from_r)?;
            let mut skatch_text = std::fs::read_to_string(&cfg.skatch).unwrap_or_default();
            if !skatch_text.trim_end().is_empty() {
                skatch_text.push_str("\n\n");
            }
            skatch_text.push_str(&content);
            if !skatch_text.ends_with('\n') {
                skatch_text.push('\n');
            }
            std::fs::write(&cfg.skatch, skatch_text)?;
            std::fs::remove_file(&from_r)?;
            Ok(serde_json::json!({
                "skatch": cfg.skatch,
                "root": cfg.root,
                "removed": from_r,
                "inserted": true,
            }))
        }
        Request::MoveNote { from, to_dir } => {
            let from_r = resolve_note_arg(cfg, &from)?;
            let to_dir_r = path::resolve_dir_in_root(&cfg.root, &to_dir)?;
            if from_r.parent() == Some(to_dir_r.as_path()) {
                bail!("文件已在目标目录中: {from}");
            }
            if to_dir_r == cfg.skatch.parent().map(|p| p.to_path_buf()).unwrap_or_default() {
                // skatch.md 所在目录可作目标（无特殊限制），此处仅防移动到自身
            }
            let name = from_r
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("源文件无文件名: {from}"))?;
            let to_r = to_dir_r.join(name);
            if to_r.exists() {
                bail!("目标已存在，不覆盖: {}", to_r.display());
            }
            std::fs::rename(&from_r, &to_r)?;
            Ok(serde_json::json!({ "from": from_r, "to": to_r, "root": cfg.root, "moved": true }))
        }
        Request::MoveDir { from, to_dir } => {
            let from_r = path::resolve_dir_in_root(&cfg.root, &from)?;
            let to_dir_r = path::resolve_dir_in_root(&cfg.root, &to_dir)?;
            // 防嵌套：不能把目录移进它自身或自己的子树
            if to_dir_r.starts_with(&from_r) {
                bail!("不能把目录移进它自身: {from}");
            }
            if from_r.parent() == Some(to_dir_r.as_path()) {
                bail!("目录已在目标目录中: {from}");
            }
            let name = from_r
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("源目录无名称: {from}"))?;
            let to_r = to_dir_r.join(name);
            if to_r.exists() {
                bail!("目标已存在，不覆盖: {}", to_r.display());
            }
            std::fs::rename(&from_r, &to_r)?;
            Ok(serde_json::json!({ "from": from_r, "to": to_r, "root": cfg.root, "moved": true }))
        }
    }
}

/// 去掉笔记扩展名（.md/.markdown/.txt）；无扩展名原样返回。
fn strip_note_ext(title: &str) -> String {
    let t = title.trim();
    for ext in [".md", ".markdown", ".txt"] {
        if t.to_ascii_lowercase().ends_with(ext) {
            return t[..t.len() - ext.len()].to_string();
        }
    }
    t.to_string()
}

/// 把任意可序列化结果转成 JSON 值（统一错误类型便于 `?` 传播）。
fn json_ok<T: serde::Serialize>(r: anyhow::Result<T>) -> anyhow::Result<serde_json::Value> {
    r.and_then(|v| serde_json::to_value(v).map_err(Into::into))
}

/// 解析并校验标签操作的目标路径：白名单（笔记库根目录内）+ 必须是笔记文件。
fn resolve_note_arg(cfg: &Config, user_path: &str) -> Result<PathBuf> {
    if user_path.is_empty() {
        bail!("缺少 path 参数");
    }
    let resolved = path::resolve_file_in_root(&cfg.root, user_path)?;
    if !query::is_note_path(&resolved) {
        bail!("不是笔记文件（仅支持 .md/.markdown/.txt）: {user_path}");
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anm_core::config::{McpConfig, ServerConfig};
    use anm_core::protocol::Request;

    /// 构造一个指向临时笔记库的 Config（不触碰真实 ~/.anm）。
    fn test_config(name: &str) -> Config {
        let base = std::env::temp_dir().join(format!("anm-server-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("home");
        let root = base.join("notes");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        Config {
            home: home.clone(),
            config_path: home.join("config.toml"),
            root: root.clone(),
            editor: "true".to_string(),
            skatch: root.join("skatch.md"),
            mcp: McpConfig::default(),
            server: ServerConfig::default(),
        }
    }

    /// 查询类：Dirs / Tags / FindTag 走现场扫描返回结果。
    #[test]
    fn dispatch_query_commands() {
        let cfg = test_config("query");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::write(cfg.root.join("idea/a.md"), "@ai @rust\n\n# A\n").unwrap();
        std::fs::write(cfg.root.join("b.md"), "# B\n").unwrap();

        let resp = dispatch(&cfg, Request::Dirs);
        assert!(resp.ok);
        let dirs = resp.data.unwrap();
        assert_eq!(dirs[0]["name"], "idea");

        // 总览聚合：一级目录 + 各自直接笔记
        let resp = dispatch(&cfg, Request::Overview);
        assert!(resp.ok);
        let ov = resp.data.unwrap();
        assert_eq!(ov[0]["name"], "idea");
        assert_eq!(ov[0]["notes"].as_array().unwrap().len(), 1);

        let resp = dispatch(&cfg, Request::Tags);
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap(), serde_json::json!(["ai", "rust"]));

        let resp = dispatch(&cfg, Request::FindTag {
            tags: vec!["rust".into()],
        });
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap().as_array().unwrap().len(), 1);

        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 写入类：TagMoveTop / TagAdd / InboxAppend 均在白名单内生效。
    #[test]
    fn dispatch_write_commands() {
        let cfg = test_config("write");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        let note = cfg.root.join("idea/n.md");
        std::fs::write(&note, "@b\n\n# N\n").unwrap();

        // 新增标签（只追加，不重排）
        let resp = dispatch(&cfg, Request::TagAdd {
            path: "idea/n.md".into(),
            tags: vec!["x".into()],
        });
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap()["added"][0], "x");

        // 标签行置顶（已有标签行在开头时无变化）
        let resp = dispatch(&cfg, Request::TagMoveTop { path: "idea/n.md".into() });
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap()["changed"], false);

        // inbox 追加
        let resp = dispatch(&cfg, Request::InboxAppend { text: "一条待办".into() });
        assert!(resp.ok);
        assert!(cfg.skatch.exists());

        // 越界路径被白名单拒绝（绝对路径逃逸 → "超出"；相对路径逃逸 → 不存在）
        let resp = dispatch(&cfg, Request::TagMoveTop { path: "/etc/passwd".into() });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("超出"));
        let resp = dispatch(&cfg, Request::TagMoveTop { path: "../etc/passwd".into() });
        assert!(!resp.ok);

        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 笔记内容读写：ReadNote 返回全文，WriteNote 落盘（人机通道）。
    #[test]
    fn dispatch_note_read_write() {
        let cfg = test_config("noteio");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::write(cfg.root.join("idea/a.md"), "@x\n\n# A\n正文").unwrap();

        let resp = dispatch(&cfg, Request::ReadNote { path: "idea/a.md".into() });
        assert!(resp.ok);
        assert_eq!(resp.data.unwrap()["content"], "@x\n\n# A\n正文");

        let resp = dispatch(&cfg, Request::WriteNote {
            path: "idea/a.md".into(),
            content: "@x\n\n# A\n新正文".into(),
        });
        assert!(resp.ok);
        assert_eq!(std::fs::read_to_string(cfg.root.join("idea/a.md")).unwrap(), "@x\n\n# A\n新正文");

        // 越界路径拒绝
        let resp = dispatch(&cfg, Request::ReadNote { path: "/etc/hosts".into() });
        assert!(!resp.ok);
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 新建 / 重命名：白名单内生效，绝不覆盖已有文件。
    #[test]
    fn dispatch_create_rename() {
        let cfg = test_config("crename");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::write(cfg.root.join("idea/a.md"), "# A\n").unwrap();

        // 新建（标题带 .md 后缀也归一为 .md）
        let resp = dispatch(&cfg, Request::CreateNote {
            dir: "idea".into(),
            title: "新笔记.md".into(),
        });
        assert!(resp.ok, "CreateNote: {}", resp.error.unwrap_or_default());
        let p = resp.data.unwrap();
        assert_eq!(p["path"].as_str().unwrap(), cfg.root.join("idea/新笔记.md").to_str().unwrap());
        assert!(cfg.root.join("idea/新笔记.md").exists());

        // 重名 → 拒绝（不覆盖）
        let resp = dispatch(&cfg, Request::CreateNote {
            dir: "idea".into(),
            title: "新笔记".into(),
        });
        assert!(!resp.ok);

        // 同目录重命名
        let resp = dispatch(&cfg, Request::RenameNote {
            from: "idea/a.md".into(),
            to: "idea/b.md".into(),
        });
        assert!(resp.ok, "RenameNote: {}", resp.error.unwrap_or_default());
        assert!(cfg.root.join("idea/b.md").exists());
        assert!(!cfg.root.join("idea/a.md").exists());

        // 跨目录重命名 → 拒绝
        std::fs::create_dir_all(cfg.root.join("ref")).unwrap();
        let resp = dispatch(&cfg, Request::RenameNote {
            from: "idea/b.md".into(),
            to: "ref/c.md".into(),
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("同目录"));

        // 目标已存在 → 拒绝
        std::fs::write(cfg.root.join("idea/b.md"), "# B\n").unwrap();
        std::fs::write(cfg.root.join("idea/c.md"), "# C\n").unwrap();
        let resp = dispatch(&cfg, Request::RenameNote {
            from: "idea/b.md".into(),
            to: "idea/c.md".into(),
        });
        assert!(!resp.ok);
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// skatch 总览：空行分隔段落，返回首行与全文段；root 字段随附。
    #[test]
    fn dispatch_skatch() {
        let cfg = test_config("skatch");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::write(&cfg.skatch, "- 第一条\n\n## 小节\n- 第二条内容\n- 续行\n\n").unwrap();

        let resp = dispatch(&cfg, Request::Skatch);
        assert!(resp.ok, "{}", resp.error.unwrap_or_default());
        let d = resp.data.unwrap();
        // 按行分段：空行丢弃，其余每行一段
        assert_eq!(d["segments"].as_array().unwrap().len(), 4);
        assert_eq!(d["segments"][0], "- 第一条");
        assert_eq!(d["segments"][1], "## 小节");
        assert_eq!(d["segments"][2], "- 第二条内容");
        assert_eq!(d["segments"][3], "- 续行");
        assert!(d["root"].is_string());

        // root 字段随 ReadNote 响应
        std::fs::write(cfg.root.join("idea/a.md"), "# A\n").unwrap();
        let resp = dispatch(&cfg, Request::ReadNote { path: "idea/a.md".into() });
        assert_eq!(resp.data.unwrap()["root"], cfg.root.to_str().unwrap());
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 跨目录移动：源必须存在、目标目录白名单、重名拒绝、成功后原路径消失。
    #[test]
    fn dispatch_move_note() {
        let cfg = test_config("move");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::create_dir_all(cfg.root.join("ref")).unwrap();
        std::fs::write(cfg.root.join("idea/a.md"), "# A\n").unwrap();

        // 跨目录移动成功
        let resp = dispatch(&cfg, Request::MoveNote {
            from: "idea/a.md".into(),
            to_dir: "ref".into(),
        });
        assert!(resp.ok, "{}", resp.error.unwrap_or_default());
        assert!(cfg.root.join("ref/a.md").exists());
        assert!(!cfg.root.join("idea/a.md").exists());
        assert_eq!(resp.data.unwrap()["root"], cfg.root.to_str().unwrap());

        // 已存在 → 拒绝
        std::fs::write(cfg.root.join("idea/b.md"), "# B\n").unwrap();
        std::fs::write(cfg.root.join("ref/b.md"), "# B2\n").unwrap();
        let resp = dispatch(&cfg, Request::MoveNote {
            from: "idea/b.md".into(),
            to_dir: "ref".into(),
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("已存在"));

        // 同目录 → 拒绝
        let resp = dispatch(&cfg, Request::MoveNote {
            from: "idea/b.md".into(),
            to_dir: "idea".into(),
        });
        assert!(!resp.ok);
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// skatch 抽取/并入：段落出文件、文件入 skatch，双向对称。
    #[test]
    fn dispatch_skatch_extract_insert() {
        let cfg = test_config("skx");
        std::fs::create_dir_all(cfg.root.join("idea")).unwrap();
        std::fs::write(&cfg.skatch, "- 第一条\n\n## 小节\n- 内容\n").unwrap();

        // 抽取下标 1 的段落 → idea 下新文件，skatch 剩第一条
        let resp = dispatch(&cfg, Request::SkatchExtract {
            dir: "idea".into(),
            index: 1,
        });
        assert!(resp.ok, "{}", resp.error.unwrap_or_default());
        let p = resp.data.unwrap()["path"].as_str().unwrap().to_string();
        assert!(p.ends_with("小节.md"), "{p}");
        assert!(std::fs::read_to_string(&p).unwrap().contains("## 小节"));
        let remain = std::fs::read_to_string(&cfg.skatch).unwrap();
        assert!(remain.contains("- 第一条"));
        assert!(!remain.contains("## 小节"));

        // 下标越界 → 拒绝
        let resp = dispatch(&cfg, Request::SkatchExtract { dir: "idea".into(), index: 99 });
        assert!(!resp.ok);

        // 并入：抽取出的文件 → skatch 末尾，原文件删除
        let fname = std::path::Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
        let resp = dispatch(&cfg, Request::SkatchInsert {
            from: format!("idea/{fname}"),
        });
        assert!(resp.ok, "{}", resp.error.unwrap_or_default());
        let sk = std::fs::read_to_string(&cfg.skatch).unwrap();
        assert!(sk.contains("## 小节"));
        assert!(sk.starts_with("- 第一条"));
        assert!(!cfg.root.join("idea").join(fname).exists());

        // skatch 自身并入 → 拒绝
        let resp = dispatch(&cfg, Request::SkatchInsert { from: "skatch.md".into() });
        assert!(!resp.ok);
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 令牌：配置后必须携带相同令牌；未配置则放行。
    #[test]
    fn token_gate() {
        let mut cfg = test_config("token");
        assert!(check_token(&cfg, None).is_ok());

        cfg.server.token = Some("s3cret".into());
        assert!(check_token(&cfg, None).is_err());
        assert!(check_token(&cfg, Some("s3cre")).is_err());
        assert!(check_token(&cfg, Some("s3cret")).is_ok());
        assert!(check_token(&cfg, Some("S3CRET")).is_err());
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    /// 未知/畸形请求返回失败响应而不是 panic。
    #[test]
    fn dispatch_rejects_bad_input() {
        let cfg = test_config("bad");
        let resp = dispatch(&cfg, Request::Search { keyword: "".into() });
        assert!(resp.ok); // 空关键字是合法请求，返回空结果
        std::fs::remove_dir_all(&cfg.home.parent().unwrap()).unwrap();
    }

    #[test]
    fn ok_helper_builds_success_response() {
        let resp = Response::ok(serde_json::json!({"a": 1}));
        assert!(resp.ok);
        assert!(resp.error.is_none());
    }
}
