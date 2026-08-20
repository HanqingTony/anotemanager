//! anm：通用笔记系统管理器（CLI）。

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use anm_core::{config::Config, inbox, query, tags, tree};

#[derive(Parser)]
#[command(
    name = "anm",
    version,
    about = "通用笔记系统管理器：标签、查询、浏览、inbox"
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
    /// 初始化 / 注册笔记系统
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
    /// 用配置的编辑器打开笔记
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
    /// 同步文件头部标签区（把文档中标签行统一维护到头部）
    Sync { path: PathBuf },
    /// 为文件添加标签并同步头部标签区
    Add { path: PathBuf, tags: Vec<String> },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("anm: {e:#}");
        std::process::exit(1);
    }
}

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
            TagCommand::Sync { path } => cmd_tag_sync(&path),
            TagCommand::Add { path, tags } => cmd_tag_add(&path, &tags),
        },
        Some(AnmCommand::Inbox { text }) => cmd_inbox(&text),
        Some(AnmCommand::Open { path }) => cmd_open(&path),
        Some(AnmCommand::Completion { shell }) => cmd_completion(shell),
        Some(AnmCommand::Dirs) => cmd_dirs(),
        Some(AnmCommand::TagsList) => cmd_tags_list(),
    }
}

/// 裸 `anm`：显示一级目录（TUI 后续接入）
fn cmd_default(json: bool) -> Result<()> {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("提示：先运行 `anm init <笔记系统根目录>` 注册笔记系统");
            std::process::exit(1);
        }
    };
    print_dirs(json, &cfg)
}

fn cmd_init(args: &InitArgs) -> Result<()> {
    let cfg = Config::init(&args.root, &args.editor)?;
    println!("已注册笔记系统: {}", cfg.root.display());
    println!("配置文件: {}", cfg.config_path.display());
    println!("skatch.md: {}", cfg.skatch.display());
    Ok(())
}

fn cmd_ls(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    print_dirs(json, &cfg)
}

fn print_dirs(json: bool, cfg: &Config) -> Result<()> {
    let dirs = tree::list_top_dirs(&cfg.root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&dirs)?);
    } else {
        println!("{}", cfg.root.display());
        if dirs.is_empty() {
            println!("（空）");
        }
        for d in &dirs {
            println!("  {}  {}", d.name, d.path.display());
        }
    }
    Ok(())
}

fn cmd_find(json: bool, tags: &[String]) -> Result<()> {
    let cfg = Config::load()?;
    let notes = query::find_by_tag(&cfg.root, tags)?;
    print_notes(json, &notes)
}

fn cmd_search(json: bool, keyword: &str) -> Result<()> {
    let cfg = Config::load()?;
    let notes = query::find_by_title(&cfg.root, keyword)?;
    print_notes(json, &notes)
}

fn print_notes(json: bool, notes: &[query::NoteInfo]) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(notes)?);
    } else {
        if notes.is_empty() {
            println!("（无匹配）");
        }
        for n in notes {
            let tag_str = if n.tags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", n.tags.join(", "))
            };
            println!("{}{}", n.path.display(), tag_str);
        }
    }
    Ok(())
}

fn cmd_tags(json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let tags = query::all_tags(&cfg.root)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&tags)?);
    } else if tags.is_empty() {
        println!("（无标签）");
    } else {
        for t in tags {
            println!("@{}", t);
        }
    }
    Ok(())
}

fn cmd_tag_sync(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("文件不存在: {}", path.display()));
    }
    let changed = tags::sync_header_file(path)?;
    if changed {
        println!("已同步头部标签区: {}", path.display());
    } else {
        println!("无需变化: {}", path.display());
    }
    Ok(())
}

fn cmd_tag_add(path: &std::path::Path, new_tags: &[String]) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("文件不存在: {}", path.display()));
    }
    let added = tags::add_tags(path, new_tags)?;
    if added.is_empty() {
        println!("标签均已存在，未变化");
    } else {
        println!("已添加: {}", added.join(", "));
    }
    Ok(())
}

fn cmd_inbox(text: &str) -> Result<()> {
    let cfg = Config::load()?;
    inbox::append(&cfg.skatch, text)?;
    println!("已写入 {}", cfg.skatch.display());
    Ok(())
}

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

/// 输出 shell 补全脚本
fn cmd_completion(shell: CompletionShell) -> Result<()> {
    let script = match shell {
        CompletionShell::Bash => BASH_COMPLETION,
        CompletionShell::Zsh => ZSH_COMPLETION,
        CompletionShell::Fish => FISH_COMPLETION,
    };
    print!("{script}");
    Ok(())
}

/// 输出一级目录名（每行一个），供 shell 补全使用；未初始化时静默空输出
fn cmd_dirs() -> Result<()> {
    if let Ok(cfg) = Config::load() {
        if let Ok(dirs) = tree::list_top_dirs(&cfg.root) {
            for d in dirs {
                println!("{}", d.name);
            }
        }
    }
    Ok(())
}

/// 输出全部标签名（每行一个，不带 @），供 shell 补全使用；未初始化时静默空输出
fn cmd_tags_list() -> Result<()> {
    if let Ok(cfg) = Config::load() {
        if let Ok(tags) = query::all_tags(&cfg.root) {
            for t in tags {
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
                COMPREPLY=( $(compgen -W "sync add" -- "${cur}") )
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
                _values 'tag command' sync add
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
complete -c anm -f -n '__fish_seen_subcommand_from tag' -a 'sync add'
complete -c anm -f -n '__fish_seen_subcommand_from find' -a '(__anm_tags)'
complete -c anm -f -n 'not __fish_use_subcommand; and not __fish_seen_subcommand_from find tag' -a '(__fish_complete_path)'
"###;
