# anm-core 内置 MCP：设计思路与现状

> 状态：设计整理（v2.6 时代）。与 readme.md §13 保持一致；冲突时以 readme.md 为准。

## 一、定位

MCP 是 **anm-core 的内置功能，不单独成应用**（readme §13）：anm-core 常驻服务对外两个
入口——**IPC 端点**（三个应用用，人机通道）与**内置 MCP**（agent 通道）。两者共用同一
套确定性原语，只是暴露口径不同。

核心设计意图：**把"稳定的、确定性代码能做的"与"易变的、依赖模型语义判断的"物理分离**。
MCP 只暴露数据操作原语，不承担任何场景判断；场景行为（决策/自由思维怎么调用原语、
何时 exhibit）属于 Layer 3 场景剧本（AGENTS.md 类文档），随时可改、不需要重编译。

## 二、分类原则：resource vs tool

MCP 按"**是否需要常驻在上下文里**"把原语分两类（readme §13 原文）：

| 类别 | 对应 | 特点 | 举例 |
|---|---|---|---|
| **MCP resource** | core memory / 常驻 | 小而精、session 开始即可用、**不需要专门发起查询**、默认只读 | 个人常识性事实（设备/凭据/拓扑）、标签索引摘要、当前项目概览摘要 |
| **MCP tool** | archival / 按需 | 需要 agent 主动调用才读到 | 笔记检索、全文搜索、inbox 写入、标签整理、skatch 分诊 |

**当前已落地**：resource 1 项（`anm://facts`）；tool 11 个（见下）。

## 三、设计约束（继承 readme §6/§12/§17）

1. **确定性原语**：MCP 工具直接复用 anm-core 的查询/写入原语，不做语义判断；
2. **AI 写入边界**（§6）：新增/低风险追加（新建笔记、写 inbox、**新增标签**）自主可做；
   **修改/删除已有内容**（润色正文、改标签）仅在作者显式指令下做；对已有标签唯一的
   自主操作 = **移动到文档开头**（`tag_move_top`，纯位置整理，独立原语）；
3. **写路径白名单**：所有写路径限制在笔记库根目录内（防目录穿越/符号链接逃逸）；
   **`WriteNote` 只走人机通道，MCP 不暴露**——MCP 侧没有"改写笔记内容"的工具；
4. **读取限长**：`read_note` 默认截断，防上下文爆炸；
5. **传输**：HTTP 常驻（`127.0.0.1:17371`，只绑回环）+ `--stdio` 会话（被 MCP 客户端
   spawn）；远程 agent 经 SSH 隧道复用 HTTP 端点，不对公网开放（§17 规划）。

## 四、现状盘点（v2.6，代码 crates/anm-core/src/mcp.rs）

### Resource（1 个）

| URI | 名称 | 内容来源 | 语义 |
|---|---|---|---|
| `anm://facts` | 通用事实 | 笔记库 `.agentspace/fact.md` | 人工维护、**默认只读**、永远以当前文件内容为准（现场读取不缓存）；新鲜度/失效机制见 readme §15 开放问题 1（暂不做） |

### Tool（11 个）

**查询类（只读）**

| 工具 | 参数 | 作用 |
|---|---|---|
| `ls_dirs` | 无 | 一级目录列表（浏览入口） |
| `list_tags` | 无 | 全部标签 |
| `find_tag` | `tags: string[]` | 按标签查笔记（任一命中） |
| `search` | `keyword: string` | 按标题/文件名子串查找 |
| `search_content` | `keyword, limit?` | 全文搜索，返回命中片段 + 次数，按 score 降序 |
| `read_note` | `path, limit?` | 读笔记全文（限长截断） |
| `list` | `dir?` | 目录内直接笔记（非递归） |
| `recent` | `n?` | 最近修改的笔记（默认 10 条） |

**写入类（低风险追加，自主可做）**

| 工具 | 参数 | 作用 |
|---|---|---|
| `new` | `dir, title, content?` | 新建笔记 |
| `inbox_append` | `text` | 写入默认 skatch.md（inbox 入闸） |
| `tag_add` | 笔记 + 标签 | 仅在文档开头标签区追加不存在的标签行（不动已有行） |

**整理类（唯一允许的"改已有"操作）**

| 工具 | 参数 | 作用 |
|---|---|---|
| `tag_move_top` | 笔记路径 | 已识别标签行移动到文档开头（不合并/不排序/不改语义） |

**人工通道**

| 工具 | 参数 | 作用 |
|---|---|---|
| `edit` | 笔记路径 | 用配置的编辑器打开笔记（发起人工编辑，非 MCP 改写） |

### instructions（会话级提示，随 server 声明下发）

> anm 笔记系统记忆总线（anm-core 内置）：按标签/目录/内容检索笔记，写入 inbox，
> 新增标签、整理标签行位置。所有 path/dir 参数仅在笔记库根目录内有效；对已有内容
> 的修改/删除仅在作者显式指令下进行。**开始工作时先读取资源 anm://facts**（通用事实：
> 家庭设备连接方式、VPS 凭据、当前项目等，人工维护、默认只读，永远以当前内容为准）。

## 五、当前可用于测试的基础信息（给模型的）

测试环境：anm-core 运行于 zmain（本机），MCP HTTP 端点 `127.0.0.1:17371`；
`--stdio` 模式供客户端 spawn。笔记库：`/home/tony/znote`。

### 1. 会话起点：读 `anm://facts`

fact.md（`/home/tony/znote/.agentspace/fact.md`）当前覆盖（截至 v2.6 整理时）：

- **凭据库架构**：KeePass 主库在 zmain（`~/zdata/Passwords.kdbx`，强密码）；zdb.db
  （结构化数据）；OneDrive 仅备份；LAN 弱密码直接存 fact（`tony/1qaz`）；
- **凭据总览**：SSH 密钥、PPPoE、v2ray/hysteria2（引用 KeePass 条目）、域名
  （zweblab.top）；
- **设备清单**：zmain（家庭服务器：Debian 13、192.168.0.102、IPv6 ::1000、DSH GUI
  3080、minio 9000/9001、ComfyUI、llama.cpp、代理客户端矩阵）、101（Windows，
  用户 tony，WSL 接入）、zvpsw（VPS，代理中转）；
- **网络/代理**：hysteria2 v6 默认、xray 备选、tailscale/cloudflared 状态；
- **项目索引**（若 fact.md 后续补充）：当前项目、仓库位置、运行方式。

### 2. 工具自检序列（推荐的连通性测试）

```
1. ls_dirs            → 应返回一级目录（ai/alter/idea/.../vocab）
2. list_tags          → 应返回全部标签（含 @vocab/@mastered 等）
3. read anm://facts   → 应返回 fact.md 全文
4. search_content {keyword: "决策"} → 命中片段
5. recent {n: 5}      → 最近修改笔记
6. inbox_append {text: "MCP 测试条目"} → 写入 skatch（写完可查 skatch 确认）
7. tag_move_top / tag_add → 标签整理（低风险，可逆）
```

### 3. 已知边界（测试时注意）

- `read_note` 限长截断（默认上限，长文读不全）；
- 写类工具全部限定在笔记库根目录内；MCP 无 `WriteNote`（改写已有内容要走人机通道）；
- `edit` 依赖配置的编辑器（config `[mcp] editor`，当前 vim）；
- fact.md 内容敏感（含 LAN 凭据）：仅在可信 agent 会话中使用，不对公网暴露。

## 六、后续方向（readme §15 开放问题关联，未排期）

- **resource 扩充**：标签索引摘要、当前项目概览摘要（readme §13 已列）；
- **新鲜度/失效机制**：fact 的"上次确认时间、过期标记"（开放问题 1）；
- **zdb 结构化数据查询**（zdb.db 词汇/素材等，归档后数据在 zdata）；
- **skatch 分诊写入、标签图加边**（开放问题 7 确认数据结构后）。
