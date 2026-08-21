# anm 开发日志

> 记录架构讨论、开发进度与变更。最新条目在顶部。

---

## 2026-08-20 — 接入 DeepSeek Harness

### 完成

- 新增 [examples/deepseek-harness.cordis.yml](examples/deepseek-harness.cordis.yml)：
  通过 harness 官方 MCP 客户端插件 `@deepseek-ai/dsh-mcp-client` 接入 anm-mcp（stdio），
  agent 获得 `mcp__anm__<tool>` 系列工具。
- 本机已应用：`~/.dsh/profiles/web/cordis.patch.yml` 追加 insert 条目
  （`serverName: anm`，`command: anm-mcp`，`args: ["--stdio"]`）。
- 真实环境初始化：`anm init /home/tony/znote`（root 经 canonicalize 解析为
  `/mnt/c/Users/hanqi/OneDrive/znote`——znote 是 OneDrive 的符号链接）。

### 验证

- `anm-mcp --stdio` 在真实 HOME 下 initialize + ls_dirs 正常（列出 znote 14 个一级目录）。
- patch 结构与官方 mcp 示例一致。
- 生效方式：重启 `dsh web`（或等待 HMR 重连）后，`mcp__anm__*` 工具注册到 agent 工具表。

---

## 2026-08-20 — MCP 参数入配置，默认本地 HTTP

### 完成

- **配置系统**（anm-core）：`config.toml` 新增 `[mcp]` 段：`transport`（http | stdio）、`host`、`port`。
  - 默认：`http` / `127.0.0.1` / `17371`（**默认就是本地 HTTP**）；
  - `anm init` 自动写入该段；老配置文件缺 `[mcp]` 段时回退到默认值（向后兼容）。
- **anm-mcp 启动优先级**：显式 CLI 标志（`--stdio` / `--http` / `--host` / `--port`）> 配置 `[mcp]` 段 > 默认。
  - `anm_mcp`（无参数）→ 本地 HTTP（配置默认，端点 `/mcp`）；
  - `anm_mcp --stdio` → stdio（供 Claude Desktop / Cursor / opencode spawn）；
  - `anm_mcp --http --host H --port P` → 临时覆盖。

### 验证

- 单测：anm-core 35 个、anm-mcp 7 个全部通过（含 `[mcp]` 段解析回退、resolve_mode 优先级矩阵）。
- 端到端：`anm init` 生成带 `[mcp]` 的配置；无参数启动监听 `127.0.0.1:17371`（curl initialize 200）；
  配置 `transport = "stdio"` 后无参数启动走 stdio（initialize 正常响应）。

### 说明

- stdio 不再是默认；依赖 spawn 方式的客户端需要 `--stdio` 或配置 `transport = "stdio"`。

---

## 2026-08-20 — MCP 迁移到官方 rmcp：stdio + Streamable HTTP 双传输

### 背景

MCP 首版为手写 JSON-RPC over stdio。本次按设计方向切换到官方 [rmcp](https://github.com/modelcontextprotocol/rust-sdk)（v3.1.4），
并增加 MCP 标准 **Streamable HTTP** 传输（`POST /mcp` / `GET /mcp`(SSE) / `DELETE /mcp`）。

### 完成

- **传输**：
  - `anm-mcp`（无参数）→ stdio，行为与旧版兼容；
  - `anm-mcp --http [--host H] [--port P]` → Streamable HTTP，默认绑定 `127.0.0.1:17371`，端点 `/mcp`；
  - `--host` 绑定非回环地址时需自行配合防火墙/鉴权（默认 `allowed_hosts` 仅回环，防 DNS rebinding）。
- **工具**：13 个工具全部迁移到 `#[tool]` 宏体系（参数结构 → 自动生成 JSON Schema 与参数校验）；
  安全边界原样保留（路径白名单、只读优先、`read_note` 限长截断）。
- **协议**：由 rmcp 承担握手、协议版本协商、会话管理、SSE 流式等，不再手写。
- 依赖：`rmcp`（client/server/macros/schemars + transport-io + transport-streamable-http-server）、`axum`、`schemars`、`tokio`。

### 验证

- 单测：anm-core 32 个、anm-mcp 2 个（工具注册表合法性 + 进程内握手/tools/list）全部通过。
- stdio 端到端：initialize → tools/list（13 工具）→ 越界拒绝 → search_content → new → recent。
- HTTP 端到端（curl/python）：initialize（拿 session-id）→ initialized 通知 → tools/list → tools/call（越界拒绝 + 正常调用）→ DELETE 会话，全链路通过。

### 说明

- HTTP 打破「数据不出本地」原原则：本机默认 `127.0.0.1` 仍满足本地语义；远程暴露需自行加鉴权与 TLS。
- rmcp 自动协商协议版本；工具错误走 `CallToolResult::error`（isError），协议错误走标准 JSON-RPC error。

---

## 2026-08-20 — MCP Server 补全：路径白名单 + 5 个新工具 + 协议硬化

### 完成

- **anm-core**：
  - `path`（新模块）：路径白名单。`resolve_file_in_root`（canonicalize + starts_with，防目录穿越、防符号链接逃逸）、`resolve_new_in_root`（词法校验，供尚未存在的目标路径）、`resolve_dir_in_root`（目录校验）。
  - `query`：`search_content`（全文搜索，返回 `[{file, snippet, score}]`，按命中次数降序、可限条数）、`list_in_dir`（目录下笔记列表，非递归）、`recent`（最近修改 n 条，含 mtime）、公开 `is_note_path`（笔记扩展名校验）。
  - `notes`（新模块）：`create_note`（新建笔记：目录白名单校验、标题清洗防路径逃逸、已存在绝不覆盖、缺省生成标题行）。
- **anm-mcp**：工具从 8 个扩到 13 个（新增 `search_content` / `list` / `recent` / `new` / `open`）。
  - **安全边界落地**：`read_note` / `tag_sync` / `tag_add` / `new` / `list` 的路径参数全部经 `anm_core::path` 校验，仅允许笔记库内；`read_note` / 标签操作仅接受笔记文件（.md / .markdown / .txt）。
  - `read_note` 限长截断（默认 8000 字符，返回 `truncated` / `total_chars`），防 agent 上下文爆炸。
  - JSON-RPC 硬化：未知方法返回标准 error（-32601）；无 id 的消息按 notification 处理、不回响应。

### 验证

- 单测：anm-core 32 个、anm-mcp 7 个全部通过（含越界路径拒绝、截断、新建 + 列表 + 全文搜索、协议错误码、notification 静默）。
- stdio 端到端冒烟通过：initialize → tools/list（13 工具）→ 越界 `/etc/passwd` 拒绝 → read_note 截断 → search_content 命中 → new → recent → list → open → 未知方法 error。

### 说明

- `open` 工具在 stdio 会话下仅适用于能独立开窗的编辑器（如 GUI 编辑器）；终端编辑器无法从 MCP 进程拉起交互界面。
- 下一步（roadmap 未变）：CLI / MCP 接入 daemon（TCP 优先、失败回退本地扫描）。

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
