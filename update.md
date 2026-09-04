## 2026-09-05 — anm-tauri 薄壳退役归档（纯前端 anm-web 上线）

> 里程碑：anm 前端纯前端化完成——浏览器直接访问 **http://192.168.0.102:8090** 即得完整笔记 UI
> （anm-web，apps/anm-web/，纯 HTML 单文件）。壳能力（全屏/热键/托盘）由 atray 或浏览器提供。

### 变更

- **apps/anm-web/**：从 anm-tauri renderer 独立（去薄壳/插件/布局功能，保留卡片/编辑器/
  输入框/斜杠命令/拖放/skatch；设置 = HTTP API 地址 + 令牌 + fact.md 编辑）；
  直连 anm-core 人机 HTTP API（POST /api/ipc，端口 17373，CORS 放行）
- **nginx**：192.168.0.102:8090 → /var/www/anm-web（deploy-web.sh 同步；注意文件 644）
- **归档**：apps/anm-tauri 删除（git tag `anm-tauri-final` 保留最终状态）；
  101 上的 anm-tauri 实例不再需要（可用 atray 承载 anm-web 或浏览器直访）
- anm-core：新增 http_api.rs（人机 HTTP API，Envelope 同构 TCP IPC）

### 使用

- 浏览器打开 http://192.168.0.102:8090（首次可在左下角设置里改 API 地址/令牌）
- 全屏 + 热键需求：把 http://192.168.0.102:8090 注册进 atray 的 web 应用

## 2026-09-05 — 纯前端化第一步：anm-core 人机 HTTP API + 前端双通道

> 方向（作者确认）：anm 前端纯前端化（浏览器可直接跑），壳能力由 atray/浏览器提供，
> anm-tauri 薄壳可退役。本条目记录第一步落地。

### anm-core：人机 HTTP API（`crates/anm-core/src/http_api.rs`）

- **`POST /api/ipc`**：信封协议与 TCP IPC 完全同构（`{"token","request"}` → `{"ok","data"}`），
  复用 `dispatch`/`check_token`（同权限同白名单，含全部读写命令）
- 默认端口 **17373**（`ANM_HTTP_API_PORT` 覆盖），绑定 host 同 `[server] host`
- CORS 放行（浏览器跨源 fetch）+ 令牌鉴权（信封内，与 TCP 一致）——内网宽松模式（§17 语义不变）
- MCP HTTP（17371）不受影响；TCP IPC（17370）不受影响

### renderer：双通道（Tauri 内 invoke / 纯浏览器 fetch）

- `inTauri` 检测：有 `__TAURI__` → invoke（薄壳模式原样）；无 → fetch `http://<serverAddr-host>:17373/api/ipc`
- 壳能力（hide/setHotkey/事件）浏览器模式 no-op 降级
- 浏览器模式无头实测：15 张卡片渲染、状态正常、零 JS 错误

### 说明

- anm-tauri（Tauri 模式）不受影响，照常部署使用
- 浏览器访问：静态托管 renderer 目录（nginx / 任意静态服务器 / atray 注册）
- 下一步（未排期）：静态托管落地、anm-tauri 退役评估

# anm 开发日志

> 记录架构讨论、开发进度与变更。最新条目在顶部。

## 2026-08-28 — v2.6（Tauri 主线建立）：迁移收官 + 拖放体系 + 外部目录模式

> **里程碑**：Windows 薄壳从 win32 自绘 → Electron → Neutralino 实验 → **Tauri v2**，
> 最终确定 Tauri 为唯一主线（原生全屏覆盖任务栏 + `--force-device-scale-factor=1`
> 根治 DPI 穿透）。旧实现（win32 v2.5 已归档、Electron/Neutralino 分支已删）仅存历史。

### 架构

- **apps/anm-tauri**：Rust 后端（窗口/托盘/热键/IPC 转发/配置持久化）+ 纯 HTML 单文件前端
  （renderer/index.html，零构建）；前端经自定义协议 `http://anm.localhost` 外部加载
  （免编译迭代：改前端 → deploy-front.sh 推送即生效）；
- **拖放体系（自实现 pointer 拖拽）**：文件行 → 目录卡 = MoveNote；skatch 段落行 /
  编辑器内段落 → 目录卡 = SkatchExtract；**子目录行 → 目录卡 = MoveDir（新原语，
  目录树移动，防自嵌套）**；拖影 + 目标高亮；
- **卡片布局**：点击标题栏置顶（z-index）；默认网格 skatch 占位 + 行高按需压缩
  （卡多时可见行数自适应，零重叠）；卡片最大高度 ≤ 屏幕一半；
- **对账**：单例、托盘四项菜单（显示/设置服务地址…/设置快捷键…/退出）、config.json
  持久化、启动兜底重试（根治间歇性空白）、IPC 失败错误可见；
- **anm-core**：新增 `MoveDir` 原语；其余零改动。

### 部署

- 101 环境重置（用户 hanqi → tony）：deploy.sh / deploy-front.sh 路径已更新，
  正式目录 `C:\Users\tony\anm-tauri\` + 桌面副本同步（exe + dll + renderer）；
- 交叉编译产物需与 exe 同目录部署 `WebView2Loader.dll`（动态 loader）；
- 调试端口 9222 保留（内网诊断用）。

# anm 开发日志

> 记录架构讨论、开发进度与变更。最新条目在顶部。---

## 2026-08-25 — v2.5（win32 版最终归档）：skatch 卡片/编辑器 + 拖动/预览 + RichEdit + 跨平台重构

> **里程碑**：这是 Win32 自绘外壳（anm-tray-win）的**最终版本**。功能已全部验收，
> 作者决定迁移到 **Neutralinojs**（系统 WebView + 预编译壳 + 前端全业务 + Rust 扩展
> 兜底系统集成）。本日志记录 win32 版最后阶段的全部功能；代码以 git tag `v2.5`
> 归档，未来仅作参考。

### 功能（v2.4 之后新增）

- **skatch 卡片**：暖金强调、宽度加宽、内容为 skatch 段落列表；**按行分段**
  （`\r`/`\n` 都是分隔符，一行一段，无任何标题/格式识别）；
- **skatch ↔ 编辑器联动**：从卡片打开定位到所选行；编辑器内 hover 文段 →
  卡片对应行实时指示条（RichEdit `EM_CHARFROMPOS` + CRLF 显示文本换算）；
- **内置编辑器升级**：EDIT → **RichEdit**（段落间距分隔、强制白字深底、
  `EM_SETCHARFORMAT` 在内容加载后应用）；文件名可改（`RenameNote`）；
  再次点击文件名关闭（toggle）；
- **交互模型**：仅标题栏可拖卡片；标题栏 hover 整卡高亮+置顶（hover 后保持）；
  加号按钮独立 hover；**按下即打开**（文本文件进编辑器 / 子目录行立即出子卡），
  长按松开 = 预览结束（关闭），快速松开 = 正常打开，按住移动 = 拖动；
  **文件跨卡片移动**（MoveNote）、**skatch 段落拖出成文件**（SkatchExtract）、
  **文件拖入 skatch**（SkatchInsert）；
- **子卡片**：按下即显示（预览语义）、再次点击同目录关闭、置顶保持
  （父/子卡 hover 期间）、位置以鼠标为基准偏移；
- **修复轮**：connect 超时（IPv4 优先）、激活异步化、滚动位置跨激活记忆、
  CRLF/UTF-16 偏移全部修正（中文内容不崩不错位）、`skatch_segments` 换行规范化、
  `WriteNote` 服务端统一 POSIX 换行。

### 跨平台架构沉淀（迁移的资本）

- **anm-tray-core**：`ipc.rs`（TCP 客户端 + 地址/令牌）与 `commands.rs`（斜杠命令）
  原样保留；`hotkey.rs`（字符串解析）保留；模型/卡片布局为 win32 消息模型设计，
  迁移时由前端 DOM 取代；
- **anm-core**：IPC 协议（Envelope 令牌、Read/Write/Create/Rename/Move/Skatch）、
  换行规范化、白名单校验——**全部保留，零改动**；
- **教训**：Win32 自绘 + RichEdit 的 CRLF/UTF-16 偏移问题在 DOM 前端天然不存在。

### 版本

- Cargo workspace 0.2.0 → **0.3.0**（win32 阶段完结）；git tag **v2.5**。

---

## 2026-08-25 — 新功能：skatch 卡片 + 编辑器/子卡片交互调整 + 编辑器 UI 重构

### 作者需求

1. **skatch 卡片**：外观与其它卡片大体相同、颜色稍换（暖金色强调）、宽度稍宽；
   内容为 skatch.md 单文件内的**段落**（非文件夹文件列表）；
2. **内置编辑器开关语义**：点击文件名打开、✕ 关闭之外，**再次点击同一文件名也关闭**；
   子卡片同语义：**再次点击同一子目录 = 关闭**（原为"关旧开新"）；子卡片出现位置
   改为以鼠标为基准偏移（不遮挡鼠标）；
3. **编辑器 UI**：保存与 ✕ 移到**标题栏尾**（与可修改的文件名同一行）；「打开所在位置」
   改文案为「**打开目录**」并移到**底栏末尾**（与路径同区对齐）；远程模式下路径
   不显示全路径，显示 **`<host>:<目录内相对路径>`**。

### 完成

- **anm-core**：新增 IPC `Skatch`（skatch.md 按空行分段，返回 path/root/segments）；
  `ReadNote`/`WriteNote`/`CreateNote`/`RenameNote` 响应统一随附 `root` 字段
  （供托盘计算目录内相对路径）；`query::skatch_segments` + 单测。
- **anm-tray-core**：`Card.skatch` 标记 + `build_skatch_card`（宽 = 普通卡 + 40、
  标题 "skatch"、段落 → 每段一 `File` 行（首行）、左侧垂直居中、参与滚动记忆）；
  `EditorState.root`（ReadNote 响应注入）。测试 82 全绿。
- **anm-tray-win**：
  - 激活后台线程并行拉 Overview + Skatch（skatch 失败不阻塞主界面），
    skatch 卡片追加在最上层，暖金色强调条；
  - 点击正在编辑的文件名 → 关闭编辑器；再次点击同目录子卡 → 关闭（不再重开）；
    子卡位置 = 鼠标右下方 24px（不遮挡鼠标）；
  - 编辑器布局重构：头部一行 = 文件名输入框 + 保存 + ✕；底栏 = 路径 + 「打开目录」
    （右端）；路径远程模式显示 `host:rel`（回环地址仍显示全路径）。

### 验证

- Linux 82 测试通过；交叉编译零警告；
- zmain/bak 的 anm-core（Skatch + root 字段）与 101 托盘已全部更新运行中；
- 端到端：zmain 真实 skatch 拉取 37 段、root 正确。

---

## 2026-08-25 — v2.4：卡片视觉与交互优化 + 编辑器重构 + 版本发布

### 作者需求（三轮合并）

1. 卡片优化六项：临时子卡片去紫色与主卡片同色；标题分隔线改为完全贯通；
   滚动条起点改到标题分隔线之下；标题栏（卡片宽度）调整；标题行加「+」新建按钮；
   编辑器路径挪到「打开所在位置」按钮上方、文件名可改；
2. 修复轮：卡片宽度理解修正（恢复 240，改为**收窄标题栏高度** 30→24）；
   hover 圆角阴影改**左右贯通直角条**；改名后当前激活不同步（退出/Ctrl+S 后
   后台刷新总览）；滚动位置跨激活记忆；加号必须滚到顶才显示（改**固定常驻**）；
3. 微调轮：标题文字与加号垂直居中（边框与分隔线中间）；编辑器加**贯通下分割线**，
   上分隔线同步贯通，编辑区不再覆盖功能区。

### 完成

- **卡片视觉**：临时子卡片与主卡片完全同配色（调色板按序取强调色）；
  标题分隔线从卡片左缘画到右缘；滚动条起点 = padding + header_h（标题线之下）；
  行悬停 = 左右贯通直角阴影条（右缘给滚动条留 8px）；标题栏高 24、内容垂直
  居中于"边框顶 → 分隔线"区间（文字/圆点/加号统一上移 padding/2）。
- **「+」新建笔记**：卡片右上角固定按钮（与滚动无关，始终可见，卡片悬停时高亮）；
  点击 → 输入框预填 `目录/`、光标 UTF-16 定位到末尾 → 回车经新 IPC `CreateNote`
  （标题归一化 `.md`、绝不覆盖、白名单）创建 → toast 提示 + 后台刷新总览；
  Esc 取消；命中判定加号优先于行（滚动后点击不误触发行）。
- **编辑器重构**：头部第一行改为**文件名输入框**（单行 EDIT，可改名，Enter=保存
  退出，Esc=退出）；完整路径移到「打开所在位置」按钮上方；上/下两条**贯通分割线**
  分隔头部/编辑区/功能区，编辑区高度收缩不再覆盖路径文字；保存/退出时若有改名 →
  新 IPC `RenameNote`（同目录、补扩展名、目标存在拒绝、拒路径分隔符），
  改名后头部路径即时重绘、卡片总览后台刷新。
- **修复**：`TrayState.scrolls`（目录路径 → 滚动行数）取消激活时记录、激活后
  恢复（夹紧最大滚动）；加号位置固定不随滚动消失；连接超时（2s，IPv4 优先，
  修 localhost IPv6 黑洞卡顿，见上一条日志）；激活异步化（界面秒开、数据后台拉）。

### 验证

- Linux 测试 80 通过（新增 CreateNote/RenameNote 分发、加号位置/滚动条断言更新）；
- 隔离实例端到端：创建 → 重名拒绝 → 改名 → 改名后读取 → 跨目录拒绝 ✅；
- 交叉编译零警告；zmain/bak 的 anm-core 与 101 托盘全部更新运行中。

### 版本

- Cargo workspace 0.1.0 → **0.2.0**；git tag **v2.4**（README/日志版本脉络 v1→v2.4）。

---

## 2026-08-24 — 快捷键设置 + 切换取消、菜单改名、toast/状态栏、远程内容读写、令牌认证基础

### 作者需求（本轮五件事）

1. 先整理文档（readme §16 更新、新增 §17 访问安全、feature.md 同步、本日志）；
2. 快捷键设置菜单 + 激活状态下再按快捷键取消激活（切换语义）；
3. 托盘菜单「激活」→「显示」；
4. 修改成功/失败要有提示；主界面右上方显示简单状态信息（连接状态等）；
5. 修复"can not find the path specified"：作者指出应让 core 传文件内容，
   而不是托盘读本地文件路径。

### 完成

- **文档整理**：readme §16 全面改写（菜单、快捷键、toast、状态栏、远程编辑），
  新增 §17「访问安全（现状与规划）」；§6 补充人机通道例外说明；§13 去掉托盘"规划"标注；
  feature.md 同步（新功能行 + IPC 内容读写 + 令牌说明）。
- **快捷键**：
  - 托盘菜单新增「设置快捷键…」：新窗口类 `AnmHotkeyWin`，打开时**临时注销**全局
    快捷键（避免按下当前组合触发切换而非捕获），按下组合即捕获显示（需含
    Ctrl/Alt/Shift/Win 至少一个修饰键），回车确认 / Esc 取消；确认后
    `UnregisterHotKey` + `RegisterHotKey` 换绑，注册失败（被占用）恢复旧组合并在
    对话框内红字提示；成功 toast「快捷键已更新为 …」；
  - 持久化：`config.json` 新增 `"hotkey": "Alt+Shift+Z"` 字符串（`hotkey.rs` 新增
    纯逻辑解析/格式化模块，含 4 组单测）；启动时加载，失败仅提示仍可用菜单；
  - **切换语义**：`WM_HOTKEY` 时覆盖层可见 → 取消激活（隐藏），不可见 → 显示。
- **菜单改名**：「激活」→「显示」。
- **提示与状态**：
  - 新增 **toast**（`AnmToastWin`，右上角小窗 + 绿色状态点 + 自动隐藏 3.2s，
    点击立即关闭）：设置地址生效、快捷键更新、系统打开失败等场景；
  - 覆盖层**右上角状态栏**：绿点=已连接 / 红点=未连接（取最近一次 IPC 成败，
    `ipc::last_ok()` 原子记录每次 `call()` 结果）+ 当前服务地址 + 当前快捷键。
- **远程内容读写（修复"can not find the path specified"）**：
  - 协议新增 `ReadNote` / `WriteNote`（路径白名单校验）；**MCP 不暴露 WriteNote**——
    readme §6 人机通道例外，AI 写入自主权不受影响；
  - 托盘编辑器**完全改走 IPC**：`model::enter_editor` 经 `ReadNote` 取内容、
    `save_editor` / `exit_editor` 经 `WriteNote` 保存，不再读本地文件（跨机器可编辑）；
  - 系统默认打开改用 `ShellExecuteExW` + `SEE_MASK_FLAG_NO_UI`：服务端路径在本机
    打不开时**不弹系统错误框**，改为 toast 提示并停留在覆盖层；「打开所在位置」
    同样处理。
- **令牌认证基础（安全规划落地，readme §17）**：
  - `[server] token`（可选）：配置后所有 IPC 请求必须携带相同令牌（`Envelope` =
    `{token?, request}`，老客户端不带 token 字段也兼容），常数时间比较；
  - 客户端令牌来源：托盘设置对话框新增「访问令牌」输入框（持久化到
    `config.json`）/ 环境变量 `ANM_SERVER_TOKEN` / CLI 读配置；
  - 公网暴露底线写进 readme §17：优先 Tailscale/WireGuard/SSH 隧道，不裸奔端口。

### 验证

- Linux 全量测试 79 通过（新增：协议信封往返、ReadNote/WriteNote 分发、
  token 门禁、hotkey 解析 4 组）；
- 端到端：真实服务 ReadNote/WriteNote 读写成功、越界路径拒绝、
  无 token 字段的老格式兼容；HOME 覆盖起第二个实例实测 token 门禁
  （无令牌拒绝 / 错令牌拒绝 / 对令牌放行）；
- 交叉编译零警告零错误；exe 已部署 101 桌面（interop 冒烟 8s 存活无崩溃）；
  zmain 与 bak 的 anm-core 均已更新为含新协议的新版并重启。

---

## 2026-08-21 — 设置对话框修复：输入框不可见 + 配置乱码防御

### 作者反馈与修复

- **"当前生效"出现乱码**：根因是 101 上 `config.json` 被手动编辑，地址前混入了
  `311阿道夫1k3nivadf`（文件内容即垃圾，非渲染问题）。已把文件重写为干净内容。
- **设置对话框不能输入**：设置输入框创建时缺 `WS_VISIBLE`、打开时也未 ShowWindow——
  输入框一直不可见。修复：创建即可见 + 打开时 `SWP_SHOWWINDOW` + ShowWindow + SetFocus；
  另在对话框里给输入框画了圆角边框（视觉上是输入字段）。
- **防御**：`validate_addr` 增加主机名字符校验（仅允许字母/数字/./-/_:），
  手动编辑混入的非 ASCII 乱码会被拒绝加载；含对应单测。

### 验证

- 交叉编译零警告零错误；Linux 72 测试全绿；配置已重写干净；已部署 hanqi 桌面。

---

## 2026-08-21 — 托盘「设置服务地址」+ 跨机运行（anm-core 在 zmain、托盘在 101）

### 背景

作者目标：anm-core 跑在 zmain（192.168.0.102），anm-tray-win 跑在 101 的 Windows。

### 完成

- **托盘菜单新增「设置服务地址…」**：自绘深色圆角对话框（第 4 个窗口类）——
  预填当前地址、格式校验（主机:端口）、错误红字提示、Enter=确定 / Esc=取消、
  Web 风格自绘按钮（取消/确定，悬停态）；持久化到
  `%APPDATA%/anm-tray-win/config.json`；
- **核心 ipc 支持运行时覆盖**：`set_server_addr_override`（优先级：覆盖 > 环境变量
  `ANM_SERVER_ADDR` > 默认 127.0.0.1:17370）+ `validate_addr` 校验（含单测）；
- **zmain 服务**：`[server] host` 改为 `0.0.0.0`（跨机访问前提），重启 anm-core，
  实测 101→zmain 的 Overview 返回 13 张卡片；
- **101 托盘已预置** `config.json` → `192.168.0.102:17370`。

### 验证

- 交叉编译零警告零错误；Linux 72 测试全绿（新增地址校验/覆盖优先测试）；
- 跨机链路实测：bak(101) → zmain(192.168.0.102):17370 ✓；interop 冒烟进程存活。

---

## 2026-08-21 — anm-tray-win 细节修复 + 卡片外观重设计

### 功能修复（作者实测反馈）

1. **子卡片开关**：同一子目录已存在临时卡片时，先移除旧的再开新的（按服务端规范化路径比较）。
2. **滚轮方向**：向上滚 = 看更早内容（原方向反了，取反）。
3. **编辑器不换行**：根因是 EDIT 的换行行为在**创建时**由样式决定，运行时
   SetWindowLongPtrW 切换样式不生效——改为**两个独立 EDIT 控件**（单行 launcher /
   多行编辑器，创建即带 ES_MULTILINE 自动换行），按模式切换显隐。
4. **光标是方块图标**：`WNDCLASSW.hCursor` 误用 `LoadIconW(IDC_ARROW)`（加载了图标
   而非光标），改为 `LoadCursorW`。
5. **卡片外观重设计**（Web 风格）：圆角卡片（10px）、右下错位阴影模拟层次、
   每卡一个强调色（6 色调色板循环）+ 左侧圆角强调条、标题带渐变（顶部亮→底部暗）、
   标题圆点 + 强调色文字 + 分隔线、文件行小圆点弱化文字、子目录 ▸ 绿行、
   悬停圆角高亮、细圆角滚动条；输入框窗口深色主题（深底白字 EDIT +
   WM_CTLCOLOREDIT + 细边框）。

### 验证

- 交叉编译零警告零错误；Linux 70 测试全绿；interop 冒烟进程存活；已部署 hanqi 桌面。

---

## 2026-08-21 — 托盘架构规整（anm-tray-core 共享核心）+ 五功能落地

### 架构规整（为手机/Linux 版本铺路）

- **anm-win-tray → anm-tray-win**，并新增共享核心 crate **anm-tray-core**（纯逻辑，
  无窗口系统依赖）：卡片布局/命中/拖动/滚动、状态模型（TrayState/DragState/
  EditorState/Action）、命令（anw + 斜杠）、IPC 客户端；
- anm-tray-win 只保留 **Windows 外壳**：窗口/渲染/托盘/热键/编辑器 UI/路径转换；
- 未来 **anm-tray-wayland / anm-tray-android** 只写外壳，复用 anm-tray-core；
- 核心全部可在 Linux 直接单测（本轮 70 个测试全绿）。

### 五个功能（对照作者排期）

1. **段落标签**（anm-core）：行首一个或多个 `@xxx` + 空白/缩进（含全角空格）→
   段落标签，仅可出现在段落开头；整行纯标签改称**文本标签**；extract_tags 合并两者。
2. **卡片美化与滚动**：细滚动条 + 滚轮（WM_MOUSEWHEEL 滚动光标下卡片）；目录头
   分隔线、子目录 ▸ 绿行、临时卡片紫调边框；GUI 框架迁移（egui）留作可选后续。
3. **内置临时编辑器**：点击文本条目 → 中央窗口变多行编辑框（滚动、Ctrl+S 保存、
   Esc 退出自动保存、「打开所在位置」按钮开资源管理器到所在目录）；非文本退化系统默认。
4. **斜杠命令**：/help /inbox|/anw /find /search /tags /ls /open，与 anm 命令一致。
5. **临时子卡片**：卡片显示子目录，点击后原地 +40px 生成临时卡片（紫调边框），
   可继续嵌套；取消激活清除、不写入 layout.json。

### 验证

- Linux：70 个测试全绿（新增段落标签/滚动/子卡片/斜杠命令单测）；
- 交叉编译零警告零错误（anm-tray-win.exe，PE32+ GUI）；interop 冒烟进程存活；
- 已部署 hanqi 桌面（旧 anm-win-tray.exe 已清理）。

---

## 2026-08-21 — anm-win-tray v2.3：输入框置顶修复（作者验收通过）

### 问题与修复

- **现象**：拖动卡片后，中央输入框变成与背景一样的灰色（半透明变暗层盖住了输入框）。
- **根因**：点击变暗层（开始拖动的那一下）会把变暗层提升到顶层 z 序，压住输入框窗口；
  半透明层盖在其上，视觉上输入框"变灰"。
- **修复**：
  - 变暗层处理 `WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE`——点击不激活、不提升；
  - 新增 `raise_input_window()`：拖动按下/松开后显式把输入框窗口
    `SetWindowPos(HWND_TOPMOST, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE)` 提回变暗层之上。

### 验收

- 作者实机验收通过：单例 / Alt+Shift+Z / 变暗+纯色卡片 / 卡片拖动+位置记忆 /
  anw 输入框 / 点击打开 / 托盘菜单 全部正常。
- 期间还排掉一个环境坑：Smart App Control（强制模式）拦截未签名 exe（见下一条目），
  作者已关闭该功能。

---

## 2026-08-21 — Windows 侧部署受阻：Smart App Control 拦截未签名 exe

### 现象与定位

新构建的 `anm-win-tray.exe` 在 hanqi 的 Windows 上无法启动：WSL interop 报
`Invalid argument`，cmd 直接启动报"被组织 Device Guard 策略阻止"。

- exe 校验和与本地构建**完全一致**（排除上传损坏）；PE 结构正常（PE32+ GUI，
  21 节）；notepad 等系统程序可正常启动（排除 interop 环境问题）；
- 事件日志（Microsoft-Windows-CodeIntegrity/Operational，3077/3118）：
  **Smart App Control Block**；
- 注册表 `HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy\
  VerifiedAndReputablePolicyState = 1`（强制模式）——拦截所有未签名/无信誉
  可执行文件；v2.1/v2.2 能跑说明 SAC 是最近才进入强制模式的（Windows 更新）。

### 结论与约定

- **不是代码/构建问题**，是 Windows 11 安全策略（Smart App Control）。
- 作者决定：**关闭 Smart App Control**（永久性，设置 → Windows 安全中心 →
  应用和浏览器控制 → 智能应用控制 → 关闭）。
- 约定：以后每次重新交叉编译部署 exe，若遇"进程起不来"，先查
  CodeIntegrity 事件日志确认是否 SAC 拦截，再排查代码。

---

## 2026-08-21 — anm-win-tray v2.2：修复拖动保存/输入框失效 + 输入框改为 anw

### 作者反馈的三个问题

1. **位置不能保存**：`save_positions()` 只写文件、不更新内存位置表——下次激活用的
   还是启动时加载的空表。修复：保存时同步写回 `st.positions`；另外拖动判定从"单次
   事件位移"改为"相对按下点的累计位移"（慢速拖动也能正确识别）。
2. **拖动后输入框失效**：变暗层窗口缺 `WS_EX_NOACTIVATE`——点击卡片开始拖动时激活了
   变暗层，输入框窗口失活、键盘进不去。修复：变暗层加 `WS_EX_NOACTIVATE`（点击/拖动
   不抢输入框焦点）。
3. **输入框改为 anw 语义**：回车直接把文本追加到 skatch.md（InboxAppend），删除
   find/search/tags/ls/help 命令分发（命令能力由卡片承担）；写成功后清空输入框，
   显示"已写入 skatch.md"，方便连续快速记录。

### 验证

- 交叉编译零警告零错误；Linux 64 测试全绿；interop 冒烟进程存活；已部署 hanqi 桌面。

---

## 2026-08-21 — anm-win-tray v2.1：修复松开/点击崩溃 + 卡片纯色

### 根因（作者实测反馈的两个崩溃）

- **松开鼠标退出 / 点击文件崩溃**：同一个 bug——`ReleaseCapture()` 会**同步**发送
  `WM_CAPTURECHANGED` 到窗口，其处理函数要读 OVERLAY，而调用点正持有 `borrow_mut`
  → RefCell 重入 panic → 进程秒退（又是"借用期不重入"规则的漏网之鱼）。修复：
  `on_lbuttonup` 先取出拖动状态、释放借用，再 `ReleaseCapture()`；`SetCapture` 同样
  移出借用期。点击文件与拖动共用该路径，一并修好。
- **卡片半透明**：原方案 `LWA_ALPHA` 是整窗统一透明度，卡片也变半透明。改为
  **逐像素合成**：`UpdateLayeredWindow(AC_SRC_ALPHA)` + 32bpp DIB——背景纯黑像素
  置 `DIM_ALPHA`（全局变暗），卡片/文字像素置 255（纯色不透明）；渲染从 WM_PAINT
  改为显式 `render_overlay()`（激活/悬停/拖动/结果变化时调用）。

### 验证

- 交叉编译零警告零错误；Linux 64 测试全绿；interop 冒烟进程存活（exit 124）。
- 已部署 hanqi 桌面。

---

## 2026-08-21 — anm-win-tray v2：透明变暗背景 / 单例 / 快捷键调整 / 卡片拖动记忆

### 完成（作者实机验收后提出的四项调整）

- **背景透明但全局变暗**：覆盖层拆为两个窗口——全屏 `WS_EX_LAYERED` + `LWA_ALPHA`（160/255）
  半透明变暗层（GDI 绘制卡片），与居中的**不透明输入框窗口**分离（保证文字清晰）。
- **单例**：命名互斥体 `Local\anm-win-tray`，第二个实例启动即退出（实测 0.077s 退出码 0）。
- **快捷键**：`Shift+Alt+8` → **`Alt+Shift+Z`**。
- **卡片拖动 + 位置记忆**：按住卡片拖动（SetCapture 捕获，位移 <4px 判定为点击）、
  越界夹紧；抬起时位置保存到 `%APPDATA%/anm-win-tray/layout.json`，下次激活按目录名
  覆盖默认布局。`cards.rs` 新增 `translate_card`（纯几何，可单测）。

### 验证

- Linux 64 个测试全绿（新增 translate_card 平移/夹紧测试）；
- 交叉编译零警告零错误；WSL interop 冒烟：单例生效、进程存活；
- 已部署 hanqi 桌面（覆盖前先 taskkill 旧实例——旧实例占住文件是 /mnt/c 上传
  Permission denied 的根因，顺带确认了单例的必要性）。

---

## 2026-08-21 — anm-win-tray：Windows 托盘薄壳（v1 完成）

### 背景

作者选定 **GNU 交叉编译路线**（rustup target `x86_64-pc-windows-gnu` + MinGW），
从 WSL 直接产出 Windows 可执行文件；并给出托盘需求：全局快捷键呼出覆盖层、
目录卡片环绕、点击打开、点击空白取消、托盘菜单激活/退出。

### 完成

- **新 crate `anm-win-tray`**（workspace 第 3 个成员；Linux 上构建为提示占位二进制）：
  - `cards.rs`：卡片环绕布局与命中测试（纯几何逻辑，Linux 可单测）；
  - `wslpath.rs`：WSL 路径 → Windows 路径（`/mnt/c/...` → `C:\...`，可单测）；
  - `ipc.rs`：IPC 客户端（默认 `127.0.0.1:17370`，环境变量 `ANM_SERVER_ADDR` 可覆盖——支持跨机连 192.168.0.101 的 WSL）；
  - `win.rs`：纯 Win32（windows-sys 0.59，无 GUI 框架）——隐藏主窗（热键 + 托盘）、
    全屏置顶覆盖层（GDI 绘制卡片 + EDIT 输入框子类化）、右键菜单（激活/退出）；
  - **应用图标**：`assets/anm.ico`（16/32 双尺寸，程序化生成）+ `build.rs` 用 windres
    嵌入（零新增依赖），窗口类与托盘图标共用资源 id 1。
- **功能对照需求**：
  - 全局快捷键 `Shift+Alt+8` 呼出；托盘左键/菜单「激活」同样呼出；
  - 覆盖层：屏幕中央单行输入框（Enter 提交 anm 命令：inbox/anw、find、search、tags、ls、help；
    Esc 取消），一级目录及其直接笔记以独立卡片环绕（悬停高亮、超多文件截断为"还有 N 条"）；
  - 点击卡片目录头/文件行 → 系统默认方式打开（经 wslpath 转换路径）并取消激活；
    点击空白 → 取消激活；
  - 托盘右键菜单：激活 / 退出。
- **协议扩展**：IPC 新增 `Request::Overview` 聚合原语（一级目录 + 各自直接笔记，
  readme §12 首个聚合原语落地），server 分发 + 测试。
- **文档**：feature.md 新增「构建与交叉编译（代码规范）」章节（GNU 路线命令 + Windows 侧运行前提）；
  readme §16 更新为 v1 已实现。

### 验证

- Linux：`cargo build --workspace` + 63 个测试全绿（含卡片布局 4 + 路径转换 3）；
- 交叉编译：`cargo build --target x86_64-pc-windows-gnu -p anm-win-tray` 零警告零错误，
  产物 `target/x86_64-pc-windows-gnu/debug/anm-win-tray.exe`（PE32+ x86-64，file 验证）。
- **实机修复（经 bak 的 WSL interop 远程定位）**：首版启动即崩（"RefCell already mutably
  borrowed"）——Win32 重入问题：`SetWindowTextW`/`SendMessageW`/`GetWindowTextW` 会同步
  重入 EDIT 子类化过程，而它要读 OVERLAY 状态，外层 `borrow_mut` 未释放即 panic。
  修复规则：**持有 OVERLAY 借用期间绝不发送可重入的消息**（先取句柄/状态 → 释放借用 →
  再调用），应用于 run/activate/deactivate/submit/on_click 五处；顺带切到 GUI 子系统
  （双击不再闪控制台）。修复后经 interop 验证进程存活（timeout 5s 退出码 124）。
- **待实机测试**（需作者配合，Windows 侧）：托盘/热键/覆盖层交互、点开文件（路径转换）、
  命令提交、服务地址（127.0.0.1 转发或 `ANM_SERVER_ADDR` 跨机）。

### 说明

- 网络环境极慢（本地代理 ~34KB/s）：`cargo fetch` 多次超时，windows-sys 0.59/0.60/0.61 三版
  与 rustup windows-gnu 目标均为手动/重试补齐，后续构建已全部缓存。
- 已知限制：覆盖层按主显示器居中（未做多显示器）；托盘图标暂用系统默认图标；
  非 `/mnt` 路径不做 `\\wsl.localhost` 映射（当前 znote 全部在 /mnt/c 下，无影响）。

---

## 2026-08-21 — MCP resource：通用事实（anm://facts）

### 背景

作者决定实现"跨 agent 记忆"的第一块：**通用事实**（家庭网络设备连接方式、VPS 凭据、当前项目等），
让所有接入 anm MCP 的 agent 在会话开始即可获得，不依赖工具描述或提示词策略。

### 完成

- **存放位置（作者拍板）**：`znote/.agentspace/fact.md`，人工维护、AI 只读；**永远以当前读取的文件内容为准**（不做新鲜度/过期标记，readme §15 开放问题 1 暂不展开）。
- **anm-core（mcp 模块）**：
  - 新增 resource `anm://facts`：`resources/list` 返回一个资源（text/markdown），`resources/read` 现场读取 `.agentspace/fact.md`（不缓存）；未知 URI / 文件缺失 / 未初始化均返回带原因的 `resource_not_found`；
  - `AnmServer` 从单元结构体改为持有**会话配置快照**（stdio/HTTP 会话创建时加载一次），13 个工具与资源共用同一份配置；
  - server `instructions` 增加"开始工作时先读取 anm://facts"（第二通道保险，兼容不主动加载资源的客户端）。
- **测试**：新增 2 个进程内协议测试（list+read 全链路、文件缺失报错），bin 测试 11 → 13。
- **文档**：readme §13 记录已落地实例；feature.md 资源行更新。

### 说明

- "客户端是否在会话开始主动加载 resource"取决于客户端策略：Claude Desktop 类客户端会主动枚举加载；行为验证以实际客户端（DeepSeek Harness）实测为准——新开会话直接问一个只有 fact.md 里才有答案的问题。

---

## 2026-08-21 — 架构重写：一核心三应用（anm-core 常驻服务）

### 背景

与作者确认理想架构：**anm-core 作为长期后台运行的服务**，anm / anw / anm-win-tray 三个应用
经 IPC 访问它；MCP 是 anm-core 的内置功能（HTTP 常驻端点 + stdio 会话），不再单独成应用。
（此前"anm-daemon + anm-mcp"的两壳架构作废。）

### 完成

- **crate 归并**：删除 `anm-daemon`、`anm-mcp` 两个 crate；workspace 只剩 `anm-core` + `anm-cli`。
- **anm-core（lib + bin）**：
  - lib：确定性逻辑（config / tags / query / path / notes / inbox / tree）+ 新增 `protocol`（IPC 请求/响应类型，服务端与客户端共用）；
  - bin（anm-core 服务）：`watch`（文件监听）+ `server`（IPC：TCP + JSON 行）+ `mcp`（内置 MCP，自 anm-mcp 迁入，13 工具）三模块并行常驻；
  - `anm-core --stdio`：只跑一个 MCP stdio 会话（供客户端 spawn）；`--http [--host H] [--port P]` 覆盖 MCP 端点；
  - 配置新增 `[server]` 段（IPC 端点，默认 127.0.0.1:17370）。
- **anm-cli（lib + bin：anm / anw）**：重写为 IPC 客户端——查询/写入全部经 `client` 模块转发服务；仅 `init` / `open` / `completion` 为本地动作；`_dirs` / `_tags` 服务不可用时静默空输出。
- **MCP spawn 配置**：`anm-mcp --stdio` → `anm-core --stdio`（examples 已更新）。
- **文档**：readme §0/§9/§10/§13/§14/§16 重写为"一核心三应用"；feature.md 定位/功能表/架构同步；diagrams 重画。

### 说明

- anm / anw 依赖常驻服务：服务未启动时命令报错并提示先运行 `anm-core`；"自动拉起"列入 roadmap。
- anm-win-tray（Windows 托盘）仍未实现，规划不变（经 TCP 连接 anm-core 服务）。

---

## 2026-08-21 — 按新 readme 对齐（文档 + 代码）

readme.md 重写为需求文档（设计理念 + 架构与规格）后，本次把代码与其它文档对齐到新 readme，一切以新 readme 为准。

### 完成

- **readme 修正**：
  - 补回 §16「Windows 薄壳」定位（旧版有、新版漏掉的定位原则，经确认保留）；
  - 修正 §11 与 §6 的矛盾：润色/改写已有正文属于"修改已存在内容"，自主状态下一律禁止，以 §6 为准。
- **代码（现场扫描默认，readme §9/§10）**：
  - 删除持久索引默认路径：移除 `anm-core/src/index.rs`（JSONL 落盘）与 `Config.index_path`；
  - anm-daemon 回归「仅文件监听」：移除索引落盘与 TCP 查询服务（查询一律现场扫描）；
  - 标签原语拆分（readme §11）：`sync_header` → `move_tag_lines_to_top`（仅重排标签行位置，不合并/排序/去重/改写）；`add_tags` 只做新增，不再重排已有标签行；
  - MCP `tag_sync` → `tag_move_top`；CLI `tag sync` → `tag move`；三个 shell 补全脚本同步。
- **文档对齐**：
  - feature.md 重写（定位改为个人定制项目、服务 znote；删除索引落盘/增量/IPC/自动拉起等与 readme 冲突的功能行；补充 AI 自主权、标签图、resource/tool、场景剧本层）；
  - update.md roadmap 重排（移除「CLI 接入 daemon（索引/TCP）」「索引增量更新」，新增面板/resource/标签图/聚合原语/提醒等条目）；
  - diagrams/anm-architecture.json 更新为五层架构（html 为 archify 生成的产物，需重新渲染）。

### 说明

- 本机无 Rust 工具链，代码改动未经 cargo 编译/单测验证，需在具备工具链的环境跑 `cargo test --workspace`。
- 「多系统支持（未来）」从 feature.md / roadmap 移除：新 readme 定位为服务单一 znote 的个人项目，未再规划多笔记系统注册。

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
- [x] anm-core 服务（文件监听 + IPC + 内置 MCP：HTTP 常驻 + stdio 会话；13 个工具）
- [ ] 服务自动拉起（anm / anw 调用时确保 anm-core 在运行）
- [ ] TUI / 面板（ratatui；层级目录 + 编辑器打开；决策场景开场面板：working 现状、problem 列表、待分诊条目，完全不接 AI 可用）
- [x] MCP resource：`anm://facts` 通用事实（← 笔记库 `.agentspace/fact.md`，默认只读）
- [ ] MCP resource：标签索引摘要、当前项目概览摘要（派生类资源）
- [ ] 标签图（共现关系 + 可视化面板；数据结构先与作者确认，见 readme §15 开放问题 7）
- [ ] 聚合原语（组合查询）
- [ ] 笔记追加原语（自主允许的"低风险追加"类写入）
- [ ] anm-core 服务定时 / 条件提醒
- [ ] 头部标签区格式定稿（当前：标签行置顶 + 空行分隔）
- [ ] 自定义 skill（AI 协助 TUI 自定义，既定规划）
- [x] Windows 托盘 anm-win-tray（薄壳 + TCP 连接 anm-core 服务，见 readme §16；作者验收通过）
- [x] **段落标签**：段落开头（行首）一个或多个 `@xxx` + 任意全/半角空格或缩进 → 段落标签，仅可出现在开头；整行纯标签改称文本标签（识别与提取已实现；作用域/检索/自主权联动待细化）
- [x] **卡片滚动与美化**：细滚动条 + 滚轮滚动；目录头分隔线、子目录 ▸ 行、临时卡片紫调边框；GUI 框架迁移（egui/eframe）为可选后续项
- [x] **内置临时编辑器**：点击文本条目 → 多行编辑框（滚动、Ctrl+S、Esc 自动保存、「打开所在位置」按钮）；非文本退化系统默认
- [x] **斜杠命令**：/help /inbox|/anw /find /search /tags /ls /open，与 anm 命令一致
- [x] **临时子卡片**：卡片显示子文件夹，点击生成临时卡片；取消激活不保留
