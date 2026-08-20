# anm (anotemanager)

> 通用笔记系统管理器 —— 兼容管理任意以「文件夹 + 文本文件」组织的笔记系统。

anm 的核心形态是 **CLI** 与 **MCP Server**：人对同一份数据通过 CLI 操作，AI agent 通过 MCP 操作。二者共享同一核心逻辑与同一持久层。

## 设计原则

| # | 原则 | 说明 |
|---|------|------|
| 1 | **人机共享硬盘** | 同一块存储，对人是第二内存（笔记），对 agent 是第一内存（记忆）。anm 是这条记忆总线的操作员 |
| 2 | **通用性** | 不绑定任何特定笔记结构：只要由文件夹 + 文本文件组织，anm 就能管理。znote 只是被管理的系统之一 |
| 3 | **Linux 为根基** | anm 以 Linux 作为根基平台 |
| 4 | **非唯一入口** | 守护进程不是唯一入口：用户可随时用任何形式修改笔记目录，anm 只观察与维护索引 |
| 5 | **标签即组织** | 整行仅含一个或多个 `@xxx` 的行声明标签；标签自动维护至文档头部标签区 |
| 6 | **文档内扁平** | 文档内标签永远没有层级；层级仅存在于 anm 内部数据模型 |
| 7 | **索引落盘** | 标签反向索引落盘存储，查询不现扫全目录 |
| 8 | **后台常驻** | 守护进程常驻后台：监听变动、增量维护索引、提供 IPC |
| 9 | **逻辑集中** | 所有逻辑在 anm_core；CLI / MCP / daemon / tray 均为薄壳 |
| 10 | **核心形态** | CLI + MCP 先行；Web / WASM 后置 |
| 11 | **配置集中** | 配置位于 `~/.anm/` |
| 12 | **记忆总线** | 对 agent 而言，MCP 是取指-加载通道，不是单纯的检索接口 |
| 13 | **Windows 薄壳** | Windows 侧仅为薄壳，主要功能全部运行于 Linux / WSL |
| 14 | **TCP 连接** | Windows 托盘经 TCP 连接 WSL 内的守护进程 |

## 子工具

| 命令 | 用途 |
|------|------|
| `anm` | 主命令：笔记查询、标签管理、shell 补全、TUI 界面 |
| `anw` | 快速把后续参数写入默认 `skatch.md`（inbox 入口） |
| `anm_mcp` | MCP server，把 anm_core 的能力暴露给 AI agent |
| `anm_daemon` | 后台守护进程：常驻、监听笔记变动、维护索引、提供 IPC |
| `anm_tray` | Windows 托盘：常驻系统托盘，全局快捷键呼出交互窗口 |

## 安装

### 前置要求

- Rust 工具链 ≥ 1.85（项目使用 edition 2024）
- 依赖全部来自 crates.io，首次构建需要网络

### 从源码安装

> ⚠️ 仓库根目录是 workspace 虚拟清单（无 `[package]`），**不能** `cargo install --path .`，必须指定子 crate。

```bash
git clone <仓库地址> && cd anotemanager

# 安装 CLI（anm 与 anw 两个二进制）
cargo install --path crates/anm-cli --locked

# 按需安装其他组件：
cargo install --path crates/anm-daemon --locked   # 后台守护进程
cargo install --path crates/anm-mcp --locked      # MCP server
```

安装后 `anm` / `anw` 位于 `~/.cargo/bin/`（确认该目录在 PATH 中）。

### 首次配置

```bash
# 注册笔记系统（生成 ~/.anm/config.toml）
anm init <笔记系统根目录>

# 启用 shell 补全（bash 示例；追加到 ~/.bashrc 后新开终端生效）
echo 'source <(anm completion bash)' >> ~/.bashrc
```

## 快速上手

```bash
# 注册笔记系统（首次使用）
anm init <笔记系统根目录>

# 安装 shell 补全（bash 示例；zsh/fish 见 `anm completion --help`）
source <(anm completion bash)     # 或追加到 ~/.bashrc

# 查询笔记（按标签 / 目录 / 标题）
anm <tab>          # shell 补全：展示子命令 + 当前笔记系统的一级目录
anm find <tab>     # 补全标签
anm find ai
anm ls

# 快速记一笔（写入 skatch.md）
anw 明天检查 postgres 备份

# 进入笔记管理界面（TUI）
anm
```

> 具体行为以 [feature.md](feature.md) 为准，开发进度见 [update.md](update.md)。

## 架构

```
anm_core   (lib)  — 全部逻辑：配置、标签、查询、目录、inbox、watch
anm_daemon (bin)  — 后台守护进程：监听笔记变动、维护索引、提供 IPC（薄壳）
anm_cli    (bin)  — CLI + shell 补全 + TUI（薄壳）
anm_mcp    (bin)  — MCP server（薄壳）
anw        (bin)  — inbox 写入（薄壳）
anm_tray   (bin)  — Windows 托盘（薄壳）：全局快捷键呼出交互窗口，经 TCP 连接 WSL 内的守护进程
```

- 配置目录：`~/.anm/`（默认注册一个笔记系统）
- 标签反向索引落盘存储，不每次现扫
- 守护进程后台常驻，监听笔记目录变动；用户可随时用任何形式修改笔记目录（非唯一入口原则）
- Windows 侧（托盘）为薄壳：主要功能全部运行于 Linux / WSL，托盘经 TCP 连接守护进程

## 文档

- [feature.md](feature.md) — 功能设计文档（架构决策、标签系统、交互细节）
- [update.md](update.md) — 开发日志 / 变更记录
