//! 托盘应用状态模型与动作（平台无关）。
//!
//! 平台外壳把输入事件交给本模块的函数处理（更新 [`TrayState`]），
//! 拿到返回的 [`Action`] 后再执行平台动作（显示/隐藏窗口、ShellExecute 等）。

use std::collections::HashMap;
use std::path::Path;

use anm_core::protocol::Request;

use crate::cards::{Card, Hit, Rect};
use crate::ipc;

/// 卡片拖动状态（按下卡片开始拖动时非 None）。
#[derive(Debug, Clone)]
pub struct DragState {
    /// 被拖动的卡片下标（含临时卡片，统一在 `cards` 列表里）
    pub card: usize,
    /// 按下时命中的**可见**行（抬起时若未移动则视为点击该行）
    pub row: usize,
    /// 抓取偏移：鼠标位置 - 卡片左上角（保持拖动时卡片不跳）
    pub grab_dx: i32,
    /// 抓取偏移：鼠标位置 - 卡片左上角
    pub grab_dy: i32,
    /// 按下时的鼠标位置（累计位移判定，慢速拖动也能识别）
    pub start_x: i32,
    /// 按下时的鼠标位置
    pub start_y: i32,
    /// 是否已发生超过阈值的位移（区分"点击"与"拖动"）
    pub moved: bool,
}

/// 临时编辑器状态：内置文本编辑（点击文本条目进入）。
#[derive(Debug, Clone)]
pub struct EditorState {
    /// 正在编辑的笔记路径（**core 侧路径**，即总览/搜索返回的原始路径；
    /// 读写一律经 IPC 由 anm-core 完成，托盘不直接访问文件系统）
    pub path: String,
    /// 笔记系统根目录（ReadNote 响应随附；用于显示"目录内相对路径"）
    pub root: Option<String>,
    /// 打开时选中的 skatch 段落下标（skatch 模式编辑器定位用）
    pub skatch_index: Option<usize>,
    /// 当前编辑缓冲区内容
    pub content: String,
    /// 是否有未保存的改动
    pub dirty: bool,
}

/// 核心处理后需要平台外壳执行的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 无事
    None,
    /// 取消激活（隐藏覆盖层，清空临时状态）
    Deactivate,
    /// 用系统默认方式打开目标（**原始**路径，外壳自行做平台路径转换）
    Open(String),
    /// 进入内置临时编辑器（**core 侧路径**，内容经 IPC 拉取）
    EnterEditor(String),
    /// 生成临时子卡片：需要外壳经 IPC 拉取 `dir_path` 的 OverviewDir，
    /// 然后以 `at` 为位置构建卡片并加入状态
    OpenTempCard { dir_path: String, at: (i32, i32) },
    /// 新建笔记（卡片标题行「+」按钮）：外壳进入新建模式（输入框预填目录），
    /// 提交后经 IPC `CreateNote` 创建并刷新总览
    NewNote(String),
    /// 跨目录移动笔记文件（文件行拖到另一张卡片上松开）
    MoveNote { from: String, to_dir: String },
    /// 从 skatch 抽取段落为独立文件（段落行拖到目录卡片上松开）
    SkatchExtract { dir: String, index: usize },
    /// 把文件内容并入 skatch 末尾并删除原文件（文件行拖到 skatch 卡片上松开）
    SkatchInsert { from: String },
    /// 保存卡片位置记忆（外壳调用持久化）
    SavePositions,
}

/// 托盘应用状态（平台无关的数据）。
#[derive(Debug, Default)]
pub struct TrayState {
    /// 全部卡片（主卡片 + 临时子卡片；临时卡片 `temp = true`，排在后面）
    pub cards: Vec<Card>,
    /// 输入框矩形（布局结果，外壳据此摆放输入框窗口）
    pub input_rect: Rect,
    /// 已保存的卡片位置（目录名 → 左上角坐标，跨激活记忆；不含临时卡片）
    pub positions: HashMap<String, (i32, i32)>,
    /// 已保存的卡片滚动位置（目录路径 → 滚动行数，跨激活记忆；不含临时卡片）
    pub scrolls: HashMap<String, usize>,
    /// 置顶层顺序（目录路径，最近 hover 的在末尾 = 最上层；hover 过后保持置顶）
    pub top: Vec<String>,
    /// 临时子卡片置顶（目录路径）：打开时置顶，直到父卡片与子卡片都不再
    /// hover 才退出置顶
    pub subcard_top: Option<String>,
    /// 悬停中的卡片可见行（`None` 表示无悬停）
    pub hover: Option<Hit>,
    /// 正在拖动的卡片（`None` 表示未在拖动）
    pub drag: Option<DragState>,
    /// 最近一次输入/命令的结果文本（显示在输入框下方）
    pub result: String,
    /// 激活/操作失败提示（红色，替代 result 展示）
    pub error: String,
    /// 临时编辑器状态（`Some` 表示正处于编辑模式）
    pub editor: Option<EditorState>,
}

impl TrayState {
    /// 新建状态（注入已保存的卡片位置记忆）。
    pub fn new(positions: HashMap<String, (i32, i32)>) -> Self {
        Self {
            positions,
            ..Self::default()
        }
    }

    /// 对主卡片应用记忆位置（临时卡片不参与；未记忆的卡片留在默认布局）。
    pub fn apply_positions(&mut self, screen_w: i32, screen_h: i32) {
        for card in self.cards.iter_mut().filter(|c| !c.temp) {
            if let Some(&(x, y)) = self.positions.get(&card.title) {
                let dx = x - card.rect.x;
                let dy = y - card.rect.y;
                crate::cards::translate_card(card, dx, dy, screen_w, screen_h);
            }
        }
    }

    /// 清空临时状态（取消激活时调用）：临时卡片、编辑器、结果、悬停、拖动；
    /// 先把主卡片的滚动位置记入 `scrolls`（下次激活恢复）。
    pub fn clear_transient(&mut self) {
        for card in self.cards.iter().filter(|c| !c.temp) {
            if card.scroll > 0 {
                self.scrolls.insert(card.dir_path.clone(), card.scroll);
            } else {
                self.scrolls.remove(&card.dir_path);
            }
        }
        self.cards.retain(|c| !c.temp);
        self.editor = None;
        self.hover = None;
        self.drag = None;
        self.result.clear();
        self.error.clear();
    }

    /// 应用记忆的滚动位置（layout 之后调用；夹紧在最大滚动范围内）。
    pub fn apply_scrolls(&mut self, params: &crate::cards::LayoutParams) {
        for card in self.cards.iter_mut().filter(|c| !c.temp) {
            if let Some(&s) = self.scrolls.get(&card.dir_path) {
                crate::cards::scroll_card(card, s as isize, params);
            }
        }
    }
}

/// 进入临时编辑器：经 IPC 读取笔记内容到编辑缓冲区。
///
/// 内容由 anm-core 从服务端读取（跨机器也能编辑，不需要本地有这份文件）；
/// 失败（服务未启动 / 文件不可读等）时返回错误文本，状态不变。
///
/// `skatch_index`：从 skatch 卡片点开时传入所选段落下标（`None` = 普通笔记）。
pub fn enter_editor(
    state: &mut TrayState,
    core_path: &str,
    skatch_index: Option<usize>,
) -> Result<(), String> {
    let data = ipc::call(&Request::ReadNote { path: core_path.to_string() })
        .map_err(|e| format!("读取失败: {e}"))?;
    let content = data
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let root = data.get("root").and_then(|v| v.as_str()).map(|r| r.to_string());
    state.editor = Some(EditorState {
        path: core_path.to_string(),
        root,
        content,
        dirty: false,
        skatch_index,
    });
    state.result.clear();
    state.error.clear();
    Ok(())
}

/// 计算第 `index` 段落在文本中的起始 UTF-16 单元偏移（按 `\n` 行分段，
/// 与 core 的 skatch 分段规则一致；偏移指向行首第一个非空白字符）。
///
/// **平台无关**：偏移基于**传入文本自身的换行约定**（LF 文本每行 +1；
/// Windows 平台把内容转成 CRLF 显示文本后再传入，每行自然 +2）。越界
/// 返回 `None`。
pub fn skatch_segment_offset(content: &str, index: usize) -> Option<usize> {
    let mut offset = 0usize;
    for (i, line) in content.split('\n').enumerate() {
        let trimmed = line.trim_start();
        let lead_ws = line[..line.len() - trimmed.len()].encode_utf16().count();
        let start = offset + lead_ws;
        if i == index {
            return Some(start);
        }
        offset += line.encode_utf16().count() + 1; // 行(UTF-16) + 换行符
    }
    None
}

/// 把 UTF-16 单元下标换算为 UTF-8 字节偏移（hover 定位段落用；
/// 避免用 UTF-16 下标直接切 UTF-8 字符串导致中文边界 panic）。
pub fn utf16_to_byte(content: &str, cp: usize) -> usize {
    let mut units = 0usize;
    for (bi, ch) in content.char_indices() {
        let u = ch.len_utf16();
        if units + u > cp {
            // 光标落在字符内部（代理对中间）→ 按该字符结束处理
            return bi + ch.len_utf8();
        }
        units += u;
        if units == cp {
            return bi + ch.len_utf8();
        }
    }
    content.len()
}


/// 用外壳读到的编辑框文本同步编辑器缓冲区（标记 dirty）。
pub fn editor_set_content(state: &mut TrayState, content: &str) {
    if let Some(ed) = state.editor.as_mut() {
        if ed.content != content {
            ed.content = content.to_string();
            ed.dirty = true;
        }
    }
}

/// 保存编辑器改动（Ctrl+S，`new_name` 为头部文件名输入框的内容）；返回展示文本。
pub fn save_editor(state: &mut TrayState, new_name: &str) -> String {
    let Some(ed) = state.editor.as_mut() else {
        return "未在编辑状态".to_string();
    };
    match apply_rename(ed, new_name) {
        Err(e) => return e,
        Ok(renamed) => {
            if ed.dirty {
                match ipc::call(&Request::WriteNote {
                    path: ed.path.clone(),
                    content: ed.content.clone(),
                }) {
                    Ok(_) => {
                        ed.dirty = false;
                        format!("已保存 {}", ed.path)
                    }
                    Err(e) => format!("保存失败: {e}"),
                }
            } else if renamed {
                format!("已重命名为 {}", ed.path)
            } else {
                format!("已是最新 {}", ed.path)
            }
        }
    }
}

/// 退出编辑器（Esc / ✕）：有未保存改动时自动保存；返回展示文本（可能为空）。
pub fn exit_editor(state: &mut TrayState, new_name: &str) -> String {
    let Some(mut ed) = state.editor.take() else {
        return String::new();
    };
    match apply_rename(&mut ed, new_name) {
        Err(e) => format!("保存失败（改动未保存）: {e}"),
        Ok(_) => {
            if ed.dirty {
                match ipc::call(&Request::WriteNote {
                    path: ed.path.clone(),
                    content: ed.content.clone(),
                }) {
                    Ok(_) => format!("已自动保存 {}", ed.path),
                    Err(e) => format!("保存失败（改动未保存）: {e}"),
                }
            } else {
                String::new()
            }
        }
    }
}

/// 应用文件名改动（如有）：同目录内重命名（无扩展名时补旧扩展名），
/// 成功后更新 `ed.path`。返回是否发生了重命名。
fn apply_rename(ed: &mut EditorState, new_name: &str) -> Result<bool, String> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err("文件名不能为空".to_string());
    }
    let old_name = Path::new(&ed.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    if new_name == old_name {
        return Ok(false);
    }
    // 文件名不允许夹带路径分隔符（防目录穿越/移动）
    if new_name.contains('/') || new_name.contains('\\') {
        return Err("文件名不能包含路径分隔符".to_string());
    }
    let dir = Path::new(&ed.path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut final_name = new_name.to_string();
    // 用户没写扩展名时沿用旧扩展名
    if Path::new(&final_name).extension().is_none() {
        if let Some(ext) = Path::new(&ed.path).extension().and_then(|e| e.to_str()) {
            final_name.push('.');
            final_name.push_str(ext);
        }
    }
    let new_path = format!("{dir}/{final_name}");
    ipc::call(&Request::RenameNote {
        from: ed.path.clone(),
        to: new_path.clone(),
    })
    .map_err(|e| format!("重命名失败: {e}"))?;
    ed.path = new_path;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTF-16 偏移：中文内容下与字节偏移不同（EM_SETSEL 用 UTF-16）。
    #[test]
    fn skatch_segment_offset_is_utf16() {
        // 按行分段（LF 语义）：第一行偏移 0；
        // 第二行起始 = 第一行(3 汉字 = 3 UTF-16) + 换行(1)
        let c = "第一条\n## 小节\n内容";
        assert_eq!(skatch_segment_offset(c, 0), Some(0));
        assert_eq!(skatch_segment_offset(c, 1), Some(4));
        // "## 小节" = 5 个 UTF-16 单元（# # 空格 小 节）
        assert_eq!(skatch_segment_offset(c, 2), Some(4 + 5 + 1));
        assert!(skatch_segment_offset(c, 9).is_none());
        // CRLF 显示文本：每行行尾多一个 \r，偏移自然 +2
        let crlf = "第一条\r\n## 小节\r\n内容";
        assert_eq!(skatch_segment_offset(crlf, 1), Some(5));
        assert_eq!(skatch_segment_offset(crlf, 2), Some(5 + 6 + 1));
        // 前导空白跳过：行首第一个非空白字符
        let c2 = "  首行\n次段";
        assert_eq!(skatch_segment_offset(c2, 0), Some(2));
        // utf16→byte：中文"你"占 1 个 UTF-16、3 字节；"你好ab" 共 8 字节
        assert_eq!(utf16_to_byte("你好ab", 1), 3);
        assert_eq!(utf16_to_byte("你好ab", 4), 8); // 全部消费 → 结尾
        assert_eq!(utf16_to_byte("你好ab", 99), 8);
    }
}
