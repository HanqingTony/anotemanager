//! anm：anm-core 服务的 CLI 客户端（人机接口，一核心三应用之一）。
//!
//! 查询 / 写入子命令全部经 IPC 转发给常驻的 anm-core 服务（见 [`anm_cli::client`]）；
//! 只有四个纯本地动作不依赖服务：
//! - `init`：写 `~/.anm/config.toml`（服务启动前必须做的注册动作）；
//! - `open`：用配置的编辑器打开笔记（人机交互留在终端侧）；
//! - `completion`：输出 shell 补全脚本（静态文本）；
//! - `_dirs` / `_tags`：补全脚本的动态数据源（服务不可用时静默空输出）。

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use anm_core::config::Config;
use anm_core::protocol::Request;

use anm_cli::client;

#[derive(Parser)]
#[command(
    name = "anm",
    version,
    about = "anm-core 服务的 CLI 客户端：标签、查询、浏览、inbox"
)]
struct Cli {
    /// 机器可读输出（JSON）
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<AnmCommand>,
}

#[derive(Subcommand)]
enum AnmCommand {
    /// 初始化 / 注册笔记系统（本地写配置，需随后启动 anm-core 服务）
    Init(InitArgs),
    /// 显示笔记系统的一级目录（无参数时 `anm` 也显示）
    Ls,
    /// 按标签查找笔记（可多个，任一命中）
    Find { tags: Vec<String> },
    /// 按标题 / 文件名关键字查找
    Search { keyword: String },
    /// 列出系统中所有标签
    Tags,
    /// 标签操作
    Tag {
        #[command(subcommand)]
        cmd: TagCommand,
    },
    /// 快速写入默认 skatch.md
    Inbox { text: String },
    /// 用配置的编辑器打开笔记（本地动作）
    Open { path: PathBuf },
    /// 生成 shell 补全脚本
    Completion { shell: CompletionShell },
    /// （内部）输出一级目录名，供 shell 补全使用
    #[command(name = "_dirs", hide = true)]
    Dirs,
    /// （内部）输出全部标签名，供 shell 补全使用
    #[command(name = "_tags", hide = true)]
    TagsList,
}

/// 支持的 shell
#[derive(ValueEnum, Clone, Copy)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Args)]
struct InitArgs {
    /// 笔记系统根目录
    root: PathBuf,
    /// 默认编辑器
    #[arg(long, default_value = "vim")]
    editor: String,
}

#[derive(Subcommand)]
enum TagCommand {
    /// 将文件的标签行移动到文档开头（纯位置整理，不改变标签内容）
    Move { path: PathBuf },
    /// 为文件新增标签（仅追加新标签行，不改动已有标签行）
    Add { path: PathBuf, tags: Vec<String> },
}

/// 程序入口：解析子命令并分发到本地动作或 IPC 请求；出错时打印并退出非零。
fn main() {
    if let Err(e) = run() {
        eprintln!("anm: {e:#}");
        std::process::exit(1);
    }
}

/// 程序入口：解析子命令并分发到本地动作或 IPC 请求。
fn run() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;

    match cli.command {
        None => cmd_default(json),
        Some(AnmCommand::Init(args)) => cmd_init(&args),
        Some(AnmCommand::Ls) => cmd_ls(json),
        Some(AnmCommand::Find { tags }) => cmd_find(json, &tags),
        Some(AnmCommand::Search { keyword }) => cmd_search(json, &keyword),
        Some(AnmCommand::Tags) => cmd_tags(json),
        Some(AnmCommand::Tag { cmd }) => match cmd {
            TagCommand::Move { path } => cmd_tag_move(&path),
            TagCommand::Add { path, tags } => cmd_tag_add(&path, &tags),
        },
        Some(AnmCommand::Inbox { text }) => cmd_inbox(&text),
        Some(AnmCommand::Open { path }) => cmd_open(&path),
        Some(AnmCommand::Completion { shell }) => cmd_completion(shell),
        Some(AnmCommand::Dirs) => cmd_dirs(),
        Some(AnmCommand::TagsList) => cmd_tags_list(),
    }
}

/// 裸 `anm`：显示一级目录（TUI / 面板后续接入）。
fn cmd_default(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::Dirs)?;
    print_dirs(json, &cfg, &data)
}

/// 注册笔记系统：本地写配置，然后提示启动服务。
fn cmd_init(args: &InitArgs) -> Result<()> {
    let cfg = Config::init(&args.root, &args.editor)?;
    println!("已注册笔记系统: {}", cfg.root.display());
    println!("配置文件: {}", cfg.config_path.display());
    println!("skatch.md: {}", cfg.skatch.display());
    println!("提示：启动常驻服务 `anm-core` 后，anm / anw 才能访问笔记库。");
    Ok(())
}

/// 列出笔记系统一级目录（经 IPC）。
fn cmd_ls(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::Dirs)?;
    print_dirs(json, &cfg, &data)
}

/// 打印一级目录：JSON 原样输出；人类可读时列出"名称 + 路径"。
fn print_dirs(json: bool, cfg: &Config, data: &serde_json::Value) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        println!("{}", cfg.root.display());
        let dirs = data.as_array().ok_or_else(|| anyhow!("服务返回格式异常"))?;
        if dirs.is_empty() {
            println!("（空）");
        }
        for d in dirs {
            println!("  {}  {}", d["name"], d["path"]);
        }
    }
    Ok(())
}

/// 按标签查找笔记（任一命中；经 IPC）。
fn cmd_find(json: bool, tags: &[String]) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::FindTag { tags: tags.to_vec() })?;
    print_notes(json, &data)
}

/// 按标题 / 文件名关键字查找笔记（经 IPC）。
fn cmd_search(json: bool, keyword: &str) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::Search { keyword: keyword.to_string() })?;
    print_notes(json, &data)
}

/// 打印笔记列表：JSON 原样输出；人类可读时列出"路径 + [标签]"。
fn print_notes(json: bool, data: &serde_json::Value) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        let notes = data.as_array().ok_or_else(|| anyhow!("服务返回格式异常"))?;
        if notes.is_empty() {
            println!("（无匹配）");
        }
        for n in notes {
            let tags = n["tags"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            let tag_str = if tags.is_empty() {
                String::new()
            } else {
                format!("  [{tags}]")
            };
            println!("{}{}", n["path"], tag_str);
        }
    }
    Ok(())
}

/// 列出系统中所有标签（去重排序；经 IPC）。
fn cmd_tags(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::Tags)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        let tags = data.as_array().ok_or_else(|| anyhow!("服务返回格式异常"))?;
        if tags.is_empty() {
            println!("（无标签）");
        }
        for t in tags {
            println!("@{}", t);
        }
    }
    Ok(())
}

/// 标签行置顶：请求服务对目标笔记做纯位置整理。
fn cmd_tag_move(path: &std::path::Path) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::TagMoveTop {
        path: path.to_string_lossy().to_string(),
    })?;
    if data["changed"].as_bool().unwrap_or(false) {
        println!("标签行已移动到文档开头: {}", data["path"]);
    } else {
        println!("无需变化: {}", data["path"]);
    }
    Ok(())
}

/// 新增标签：请求服务只追加不存在的标签行。
fn cmd_tag_add(path: &std::path::Path, new_tags: &[String]) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::TagAdd {
        path: path.to_string_lossy().to_string(),
        tags: new_tags.to_vec(),
    })?;
    let added = data["added"].as_array().map(|a| a.len()).unwrap_or(0);
    if added == 0 {
        println!("标签均已存在，未变化");
    } else {
        println!("已添加: {}", data["added"]);
    }
    Ok(())
}

/// 快速写入 skatch.md：请求服务追加（本地不直接碰文件）。
fn cmd_inbox(text: &str) -> Result<()> {
    let cfg = Config::load()?;
    let data = client::call(&cfg, &Request::InboxAppend { text: text.to_string() })?;
    println!("已写入 {}", data["skatch"]);
    Ok(())
}

/// 用配置的编辑器打开笔记（本地动作，不经过服务）。
fn cmd_open(path: &std::path::Path) -> Result<()> {
    let cfg = Config::load()?;
    if !path.exists() {
        return Err(anyhow!("文件不存在: {}", path.display()));
    }
    let status = Command::new(&cfg.editor)
        .arg(path)
        .status()
        .map_err(|e| anyhow!("启动编辑器 {} 失败: {e}", cfg.editor))?;
    if !status.success() {
        return Err(anyhow!("编辑器退出码: {:?}", status.code()));
    }
    Ok(())
}

/// 输出 shell 补全脚本（静态文本，本地动作）。
fn cmd_completion(shell: CompletionShell) -> Result<()> {
    let script = match shell {
        CompletionShell::Bash => BASH_COMPLETION,
        CompletionShell::Zsh => ZSH_COMPLETION,
        CompletionShell::Fish => FISH_COMPLETION,
    };
    print!("{script}");
    Ok(())
}

/// 输出一级目录名（每行一个），供补全脚本调用；服务不可用时静默空输出。
fn cmd_dirs() -> Result<()> {
    if let Ok(cfg) = Config::load() {
        if let Ok(data) = client::call(&cfg, &Request::Dirs) {
            for d in data.as_array().into_iter().flatten() {
                println!("{}", d["name"]);
            }
        }
    }
    Ok(())
}

/// 输出全部标签名（每行一个，不带 @），供补全脚本调用；服务不可用时静默空输出。
fn cmd_tags_list() -> Result<()> {
    if let Ok(cfg) = Config::load() {
        if let Ok(data) = client::call(&cfg, &Request::Tags) {
            for t in data.as_array().into_iter().flatten() {
                println!("{t}");
            }
        }
    }
    Ok(())
}

/// bash 补全脚本
const BASH_COMPLETION: &str = r###"# anm bash completion
# 用法: source <(anm completion bash) 或追加到 ~/.bashrc
_anm_complete() {
    local cur
    cur="${COMP_WORDS[COMP_CWORD]}"
    local commands="init ls find search tags tag inbox open completion"

    if [[ ${COMP_CWORD} -eq 1 ]]; then
        local dirs
        dirs=$(anm _dirs 2>/dev/null)
        COMPREPLY=( $(compgen -W "${commands} ${dirs}" -- "${cur}") )
        return
    fi

    case "${COMP_WORDS[1]}" in
        tag)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                COMPREPLY=( $(compgen -W "move add" -- "${cur}") )
                return
            fi
            ;;
        find)
            if [[ ${COMP_CWORD} -eq 2 ]]; then
                local tags
                tags=$(anm _tags 2>/dev/null)
                COMPREPLY=( $(compgen -W "${tags}" -- "${cur}") )
                return
            fi
            ;;
    esac

    # 默认补全文件路径
    COMPREPLY=( $(compgen -f -- "${cur}") )
}
complete -F _anm_complete anm
"###;

/// zsh 补全脚本
const ZSH_COMPLETION: &str = r###"#compdef anm
# anm zsh completion
# 用法: 放入 $fpath 中（如 ~/.zsh/completions/_anm）并运行 compinit
_anm() {
    local -a commands dirs tags
    commands=(init ls find search tags tag inbox open completion)

    if (( CURRENT == 2 )); then
        dirs=("${(@f)$(anm _dirs 2>/dev/null)}")
        _describe 'command' commands
        _describe 'directory' dirs
        return
    fi

    case "${words[2]}" in
        tag)
            if (( CURRENT == 3 )); then
                _values 'tag command' move add
            else
                _files
            fi
            ;;
        find)
            tags=("${(@f)$(anm _tags 2>/dev/null)}")
            _describe 'tag' tags
            ;;
        *)
            _files
            ;;
    esac
}
compdef _anm anm
"###;

/// fish 补全脚本
const FISH_COMPLETION: &str = r###"# anm fish completion
# 用法: anm completion fish | source 或放入 ~/.config/fish/completions/anm.fish
function __anm_dirs
    anm _dirs 2>/dev/null
end

function __anm_tags
    anm _tags 2>/dev/null
end

complete -c anm -f -n '__fish_use_subcommand' -a 'init ls find search tags tag inbox open completion'
complete -c anm -f -n '__fish_use_subcommand' -a '(__anm_dirs)'
complete -c anm -f -n '__fish_seen_subcommand_from tag' -a 'move add'
complete -c anm -f -n '__fish_seen_subcommand_from find' -a '(__anm_tags)'
complete -c anm -f -n 'not __fish_use_subcommand; and not __fish_seen_subcommand_from find tag' -a '(__fish_complete_path)'
"###;
