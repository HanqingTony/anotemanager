# anm 开发日志

> 记录架构讨论、开发进度与变更。最新条目在顶部。

---

## 2026-08-19 — Shell 补全（F 决策落地）

### 完成

- **`anm completion bash|zsh|fish`**：输出对应 shell 的补全脚本。
  - bash：`source <(anm completion bash)` 或追加到 `~/.bashrc`
  - zsh：放入 `$fpath`（如 `~/.zsh/completions/_anm`）并运行 `compinit`
  - fish：`anm completion fish | source` 或放入 `~/.config/fish/completions/anm.fish`
- **隐藏命令 `anm _dirs` / `anm _tags`**：输出一级目录名 / 全部标签名（每行一个），供补全脚本调用；未初始化时静默空输出。
- **补全内容**：
  - `anm <TAB>`：子命令 + 当前笔记系统一级目录
  - `anm find <TAB>`：标签补全
  - `anm tag <TAB>`：`sync` / `add`
  - 其余位置：文件路径补全

### 验证

- bash 语法检查通过；行为验证通过（一级目录、标签、tag 子命令、文件路径四类补全全部正确）。
- 三个 shell 的脚本均可正常生成。

### 说明

- 补全脚本动态调用 `anm _dirs` / `anm _tags`，目录与标签始终与笔记系统实时一致（不缓存静态列表）。
- 含空格的目录名暂未做转义处理（后续增强）。

---

## 2026-08-19 — 首版实现：workspace + core + CLI + anw + daemon + MCP

### 完成

- **workspace 重构**：根 `Cargo.toml` 改为 workspace，成员 `anm-core` / `anm-cli` / `anm-daemon` / `anm-mcp`。
- **anm-core（lib）**：
  - `config`：`~/.anm/config.toml` 读写（root / editor / skatch 路径）
  - `tags`：标签行识别（整行仅含一个或多个 `@xxx`）、标签提取、**头部标签区同步**（C 决策落地）、添加标签
  - `index`：索引落盘 JSONL（`~/.anm/index.jsonl`，D 决策落地）
  - `query`：全量扫描 + 按标签 / 标题查询
  - `tree`：一级目录枚举（shell 补全与 TUI 用）
  - `inbox`：skatch.md 追加
- **anm-cli（bin: anm）**：子命令 `init` / `ls` / `find` / `search` / `tags` / `tag sync` / `tag add` / `inbox` / `open`；裸 `anm` 显示一级目录；支持 `--json` 结构化输出。
- **anw（bin）**：后续参数写入默认 skatch.md。
- **anm-daemon（bin）**：常驻后台；notify 递归监听笔记目录（非唯一入口，只观察不拦截）；事件防抖（500ms）后重建索引并落盘；TCP JSON 行协议（127.0.0.1:17370，`ANM_DAEMON_PORT` 可覆盖）提供 `ls` / `find_tag` / `search` / `tags`。
- **anm-mcp（bin）**：MCP（JSON-RPC 2.0 over stdio）server，8 个工具：`ls_dirs` / `list_tags` / `find_tag` / `search` / `read_note` / `write_inbox` / `tag_sync` / `tag_add`。

### 验证

- 16 个单元测试全部通过（tags / query / tree / index / inbox / config）。
- CLI 冒烟测试通过（init → ls → find → tags → anw → json → tag add → tag sync）。
- daemon 端到端通过（TCP 查询 + 外部写文件后索引自动更新）。
- MCP 端到端通过（initialize → tools/list → tools/call 全链路）。

### 说明

- 环境：WSL (deb) 中安装 rustup（stable 1.97.1，minimal profile）。
- `anm-tray`（Windows 托盘，薄壳 + TCP 连接 daemon）尚未实现，待 Windows 侧开发。
- TUI 界面（裸 `anm` 进入）尚未实现，当前裸 `anm` 显示一级目录。
- shell 补全脚本（F 决策）尚未实现。
- clippy 组件未安装（minimal profile），后续可 `rustup component add clippy`。

---

## 2026-08-19 — 架构讨论：定位与核心决策

### 背景

anm（anotemanager）原为空壳项目（`Hello, world!`）。本次讨论基于 znote 笔记系统说明书（`ztest/笔记系统说明书.md`、`ztest/给思维建模.md`）展开，确立 anm 的定位与架构方向。

### 已确立的决策

- **定位**：通用笔记系统管理器，兼容管理任意「文件夹 + 文本文件」组织的笔记系统，不绑定 znote 结构。
- **形态优先级**：CLI + MCP 先行；Web / WASM 后置。
- **架构**：`anm_core`（全部逻辑）+ `anm_cli` / `anm_mcp` / `anw` / `anm_daemon` / `anm_tray`（薄壳）。
- **安装形态**：`anm` 在 PATH；配置在 `~/.anm/`。
- **Linux 为根基**；Windows 侧（托盘）为薄壳，主要功能全在 WSL，经 TCP 连接 daemon。
- **守护进程**：常驻后台，监听笔记变动（非唯一入口原则：用户可随时用任何形式修改目标目录）。
- **核心功能**：标签管理（最重要）、笔记查询、shell 补全、TUI 界面、anw inbox 写入。
- **决策明细**（详见 feature.md 附录）：
  - B：默认注册一个笔记系统
  - C：头部保留标签区，标签自动维护至文档头部
  - D：标签反向索引落盘；文档内标签扁平，层级仅在内部数据模型
  - E：TUI 默认层级目录展示，光标移动 + 配置编辑器打开；未来可自定义；将制作 AI 自定义 skill
  - F：补全在 shell 中显示

### 产出

- `readme.md` — 项目说明（含 14 条设计原则表）
- `feature.md` — 功能设计文档（功能总览表 + 架构）
- `update.md` — 本文档

### 参考

- znote 笔记系统说明书：`~/ztest/笔记系统说明书.md`
- 给思维建模：`~/ztest/给思维建模.md`

---

## 待办（Roadmap）

- [x] shell 补全脚本（bash / zsh / fish）与安装命令
- [ ] TUI 笔记管理界面（ratatui；默认层级目录展示；编辑器打开）
- [ ] 自定义 skill（AI 协助 TUI 自定义，既定规划）
- [ ] Windows 托盘 anm-tray（薄壳 + TCP 连接 daemon）
- [ ] CLI 接入 daemon（优先走索引/TCP，失败回退本地扫描）
- [ ] 索引增量更新（当前全量重建）
- [ ] 头部标签区格式定稿（当前：单行 `@a @b @c` + 空行）
- [ ] 多系统支持（未来）
