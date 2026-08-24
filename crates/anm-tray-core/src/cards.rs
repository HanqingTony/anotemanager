//! 卡片布局：输入框居中 + 目录卡片环绕（纯几何逻辑，与平台无关）。
//!
//! 本模块可以在任意平台直接单元测试，保证覆盖层的摆放、命中、拖动、
//! 滚动逻辑在写平台外壳之前就是正确的。卡片支持：
//! - **环绕布局**：输入框居中，卡片沿圆环均匀分布；
//! - **子目录行**：每张卡片显示直接子目录（点击生成临时子卡片）；
//! - **滚动**：行数超过可见上限时支持滚动（外壳接滚轮，本模块提供
//!   滚动偏移与滚动条几何）；
//! - **临时子卡片**：`temp = true` 的卡片由点击子目录生成，不参与位置记忆。

use anm_core::query::DirOverview;

/// 矩形（整数坐标：左上角 + 宽高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// 左上角 x
    pub x: i32,
    /// 左上角 y
    pub y: i32,
    /// 宽度
    pub w: i32,
    /// 高度
    pub h: i32,
}

impl Rect {
    /// 判断点 (px, py) 是否落在矩形内（含边界）。
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// 卡片中的一行内容：目录头、子目录行或文件行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardRow {
    /// 目录头（点击打开目录）
    DirHeader,
    /// 子目录行（点击生成临时子卡片）
    SubDir { name: String },
    /// 文件行（点击打开/编辑该文件）
    File { title: String, path: String },
}

/// 一张卡片：一个目录及其直接笔记与子目录。
#[derive(Debug, Clone)]
pub struct Card {
    /// 卡片整体矩形（由布局计算；拖动时移动）
    pub rect: Rect,
    /// 目录名（同时是卡片标题）
    pub title: String,
    /// 目录绝对路径（点击目录头时打开）
    pub dir_path: String,
    /// 全部行（目录头 + 子目录行 + 文件行），滚动只在行间进行
    pub rows: Vec<CardRow>,
    /// 可见行矩形（长度 = 可见行数；首行恒为目录头，其余随滚动偏移）
    pub row_rects: Vec<Rect>,
    /// 滚动偏移：相对 `rows[1..]` 的起始下标（0 = 未滚动）
    pub scroll: usize,
    /// 最多可见行数（含目录头）
    pub max_visible: usize,
    /// 直接子目录名（与 rows 中的 SubDir 行对应）
    pub subdirs: Vec<String>,
    /// 是否临时子卡片（点击子目录生成；不参与位置记忆，取消激活即清除）
    pub temp: bool,
    /// 是否 skatch 卡片（inbox 段落卡片：暖色强调、宽度加宽、不参与目录位置记忆）
    pub skatch: bool,
}

impl Card {
    /// 可见行下标 → 全部行下标（首行固定为目录头，其余 = 1 + scroll + 偏移）。
    pub fn row_of(&self, visible_idx: usize) -> usize {
        if visible_idx == 0 {
            0
        } else {
            1 + self.scroll + visible_idx - 1
        }
    }

    /// 是否可滚动（全部行数超过可见上限）。
    pub fn scrollable(&self) -> bool {
        self.rows.len() > self.max_visible
    }

    /// 最大滚动偏移（行数差）。
    pub fn max_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.max_visible)
    }

    /// 可见行数（min(全部行, 上限)）。
    fn visible_len(&self) -> usize {
        self.rows.len().min(self.max_visible)
    }
}

/// 命中结果：指向某张卡片的某个**可见**行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// 卡片在布局数组中的下标
    pub card: usize,
    /// 可见行下标（真实行 = `card.row_of(row)`）
    pub row: usize,
}

/// 布局参数（覆盖层 UI 常量，集中于此便于调节）。
#[derive(Debug, Clone)]
pub struct LayoutParams {
    /// 输入框宽度
    pub input_w: i32,
    /// 输入框高度
    pub input_h: i32,
    /// 卡片宽度
    pub card_w: i32,
    /// 卡片目录头高度
    pub header_h: i32,
    /// 卡片每行高度
    pub row_h: i32,
    /// 卡片内边距（上下留白）
    pub padding: i32,
    /// 环绕半径占屏幕短边的比例
    pub radius_ratio: f64,
    /// 卡片最多可见行数（含目录头；超出可滚动）
    pub max_rows: usize,
}

impl Default for LayoutParams {
    /// 覆盖层默认外观参数（屏幕分辨率自适应半径，其余为经验值）。
    fn default() -> Self {
        Self {
            input_w: 420,
            input_h: 34,
            card_w: 240,
            header_h: 24,
            row_h: 20,
            padding: 8,
            radius_ratio: 0.36,
            max_rows: 12,
        }
    }
}

/// 计算整体布局：输入框居中，卡片沿圆环均匀分布。
///
/// - 输入框矩形：屏幕正中心，尺寸取 `params.input_w × input_h`；
/// - 卡片：以屏幕中心为圆心、半径 `min(w,h) × radius_ratio` 的圆环上均匀
///   分布，第一张从正上方（-90°）开始顺时针排布；卡片高度按可见行数自适应，
///   并夹紧在屏幕内；
/// - 返回 `(输入框矩形, 卡片列表)`，卡片顺序即绘制顺序（后者覆盖前者）。
pub fn layout(
    screen_w: i32,
    screen_h: i32,
    dirs: &[DirOverview],
    params: &LayoutParams,
) -> (Rect, Vec<Card>) {
    let input = Rect {
        x: (screen_w - params.input_w) / 2,
        y: (screen_h - params.input_h) / 2,
        w: params.input_w,
        h: params.input_h,
    };

    let n = dirs.len();
    let mut cards = Vec::with_capacity(n);
    for (i, ov) in dirs.iter().enumerate() {
        // 先构建（位置占位 0,0，行矩形由最终位置决定）
        let mut card = build_card(ov, (0, 0), params, false);
        // 圆环位置：正上方起始、顺时针均匀分布，按卡片实际尺寸取坐标
        let cx = screen_w as f64 / 2.0;
        let cy = screen_h as f64 / 2.0;
        let radius = (screen_w.min(screen_h) as f64) * params.radius_ratio;
        let angle = -std::f64::consts::FRAC_PI_2 + 2.0 * std::f64::consts::PI * i as f64 / n as f64;
        card.rect.x = (cx + angle.cos() * radius - card.rect.w as f64 / 2.0).round() as i32;
        card.rect.y = (cy + angle.sin() * radius - card.rect.h as f64 / 2.0).round() as i32;
        clamp_to_screen(&mut card, screen_w, screen_h);
        recompute_row_rects(&mut card, params);
        cards.push(card);
    }

    (input, cards)
}

/// 由目录总览构建一张卡片（主卡片与临时子卡片共用）。
///
/// - `at`：卡片左上角期望位置（不夹紧，由调用方决定是否 [`clamp_to_screen`]）；
/// - `temp`：是否为临时子卡片。
pub fn build_card(ov: &DirOverview, at: (i32, i32), params: &LayoutParams, temp: bool) -> Card {
    // 行内容：目录头 + 子目录行 + 文件行
    let mut rows = vec![CardRow::DirHeader];
    for sub in &ov.subdirs {
        rows.push(CardRow::SubDir { name: sub.clone() });
    }
    for note in &ov.notes {
        rows.push(CardRow::File {
            title: note.title.clone(),
            path: note.path.to_string_lossy().to_string(),
        });
    }

    // 卡片高度 = 内边距 × 2 + 目录头高度 + 其余可见行 × 行高
    let visible = rows.len().min(params.max_rows);
    let card_h = params.padding * 2 + params.header_h + (visible as i32 - 1) * params.row_h;

    Card {
        rect: Rect {
            x: at.0,
            y: at.1,
            w: params.card_w,
            h: card_h,
        },
        title: ov.name.clone(),
        dir_path: ov.path.to_string_lossy().to_string(),
        subdirs: ov.subdirs.clone(),
        rows,
        row_rects: Vec::new(),
        scroll: 0,
        max_visible: params.max_rows,
        temp,
        skatch: false,
    }
}

/// 生成 skatch 卡片：显示 inbox 文件的段落列表（每段一行，取段落首行）。
///
/// - 宽度 = 普通卡片 + [`SKATCH_EXTRA_W`]，位置在屏幕左侧垂直居中；
/// - 标题固定为 `"skatch"`；`dir_path` 为 skatch 文件路径（滚动记忆用）；
/// - 段落行复用 `CardRow::File`（点击进内置编辑器，与笔记文件行为一致）。
pub const SKATCH_EXTRA_W: i32 = 40;

pub fn build_skatch_card(
    skatch_path: &str,
    segments: &[String],
    screen_w: i32,
    screen_h: i32,
    params: &LayoutParams,
) -> Card {
    let mut rows = vec![CardRow::DirHeader];
    for seg in segments {
        // 段落可能多行：卡片行显示首行
        let first = seg.lines().next().unwrap_or("").to_string();
        rows.push(CardRow::File {
            title: first,
            path: skatch_path.to_string(),
        });
    }
    let visible = rows.len().min(params.max_rows);
    let card_h = params.padding * 2 + params.header_h + (visible as i32 - 1) * params.row_h;
    let w = params.card_w + SKATCH_EXTRA_W;
    let x = 16;
    let y = ((screen_h - card_h) / 2).max(0);
    let mut card = Card {
        rect: Rect { x, y, w, h: card_h },
        title: "skatch".to_string(),
        dir_path: skatch_path.to_string(),
        subdirs: Vec::new(),
        rows,
        row_rects: Vec::new(),
        scroll: 0,
        max_visible: params.max_rows,
        temp: false,
        skatch: true,
    };
    clamp_to_screen(&mut card, screen_w, screen_h);
    recompute_row_rects(&mut card, params);
    card
}

/// 生成临时子卡片（点击子目录行时调用）：位置为给定坐标（父卡片位置 + 偏移），
/// 夹紧屏幕内并重算可见行矩形。
pub fn build_temp_card(
    ov: &DirOverview,
    at: (i32, i32),
    screen_w: i32,
    screen_h: i32,
    params: &LayoutParams,
) -> Card {
    let mut card = build_card(ov, at, params, true);
    clamp_to_screen(&mut card, screen_w, screen_h);
    recompute_row_rects(&mut card, params);
    card
}

/// 把卡片夹紧在屏幕内（不越界）。
pub fn clamp_to_screen(card: &mut Card, screen_w: i32, screen_h: i32) {
    card.rect.x = card.rect.x.clamp(0, (screen_w - card.rect.w).max(0));
    card.rect.y = card.rect.y.clamp(0, (screen_h - card.rect.h).max(0));
}

/// 按当前 scroll 重算可见行矩形（首行目录头固定，其余从 scroll 处开始）。
fn recompute_row_rects(card: &mut Card, params: &LayoutParams) {
    let visible = card.visible_len();
    card.scroll = card.scroll.min(card.max_scroll());
    card.row_rects.clear();
    let mut cursor_y = card.rect.y + params.padding;
    for v in 0..visible {
        let real = card.row_of(v);
        let row_h = if real == 0 { params.header_h } else { params.row_h };
        card.row_rects.push(Rect {
            x: card.rect.x,
            y: cursor_y,
            w: card.rect.w,
            h: row_h,
        });
        cursor_y += row_h;
    }
}

/// 滚动一张卡片（滚轮 delta 行数），返回是否有变化。
pub fn scroll_card(card: &mut Card, delta: isize, params: &LayoutParams) -> bool {
    if !card.scrollable() {
        return false;
    }
    let new = (card.scroll as isize + delta).clamp(0, card.max_scroll() as isize) as usize;
    if new == card.scroll {
        return false;
    }
    card.scroll = new;
    recompute_row_rects(card, params);
    true
}

/// 滚动条轨道矩形（卡片右缘细条；不可滚动时返回 None）。
///
/// 起点在**标题分隔线之下**（padding + header_h），不覆盖标题行。
pub fn scrollbar_rect(card: &Card, params: &LayoutParams) -> Option<Rect> {
    if !card.scrollable() {
        return None;
    }
    Some(Rect {
        x: card.rect.x + card.rect.w - 6,
        y: card.rect.y + params.padding + params.header_h,
        w: 4,
        h: card.rect.h - params.padding * 2 - params.header_h,
    })
}

/// 卡片右上角「新建笔记」加号按钮矩形。
///
/// 位置**固定**在标题带区域（与滚动无关）——滚动到下面时加号仍然可见，
/// 因为它的语义是"在该目录新建文件"，不依赖标题行是否在视口内。
pub fn title_plus_rect(card: &Card, params: &LayoutParams) -> Option<Rect> {
    if card.row_rects.is_empty() {
        return None;
    }
    let size = 18;
    // 垂直中心对齐"边框顶 → 分隔线"区间的中部（相对原位置整体上移 padding/2）
    let y = card.rect.y + params.padding + (params.header_h - size) / 2 - params.padding / 2;
    Some(Rect {
        x: card.rect.x + card.rect.w - size - 10,
        y,
        w: size,
        h: size,
    })
}

/// 滚动条滑块矩形（按滚动比例定位；不可滚动时返回 None）。
pub fn scrollbar_thumb(card: &Card, params: &LayoutParams) -> Option<Rect> {
    let bar = scrollbar_rect(card, params)?;
    let total = card.rows.len().max(1) as i32;
    let visible = card.max_visible.max(1) as i32;
    let thumb_h = (bar.h * visible / total).max(14);
    let travel = (bar.h - thumb_h).max(0);
    let frac = if card.max_scroll() == 0 {
        0.0
    } else {
        card.scroll as f64 / card.max_scroll() as f64
    };
    Some(Rect {
        x: bar.x,
        y: bar.y + (travel as f64 * frac) as i32,
        w: bar.w,
        h: thumb_h,
    })
}

/// 命中测试：返回最上层（绘制顺序靠后）被点中的卡片可见行。
///
/// 逆序遍历卡片（后绘制的在上层优先命中），命中后逆序遍历该卡片可见行；
/// 都未命中返回 `None`（调用方据此决定"点击空白 → 取消激活"）。
pub fn hit_test(px: i32, py: i32, cards: &[Card]) -> Option<Hit> {
    for (ci, card) in cards.iter().enumerate().rev() {
        if !card.rect.contains(px, py) {
            continue;
        }
        for (ri, row) in card.row_rects.iter().enumerate().rev() {
            if row.contains(px, py) {
                return Some(Hit { card: ci, row: ri });
            }
        }
    }
    None
}

/// 平移一张卡片（拖动时用）：卡片矩形与可见行矩形同步移动，
/// 并夹紧在屏幕范围内（不会拖出屏幕）。
pub fn translate_card(card: &mut Card, dx: i32, dy: i32, screen_w: i32, screen_h: i32) {
    let nx = (card.rect.x + dx).clamp(0, (screen_w - card.rect.w).max(0));
    let ny = (card.rect.y + dy).clamp(0, (screen_h - card.rect.h).max(0));
    let ox = nx - card.rect.x;
    let oy = ny - card.rect.y;
    card.rect.x = nx;
    card.rect.y = ny;
    for r in &mut card.row_rects {
        r.x += ox;
        r.y += oy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anm_core::query::{DirOverview, NoteInfo};
    use std::path::PathBuf;

    /// 构造测试用总览数据：2 个目录，一个 1 条笔记 + 1 个子目录、一个 3 条笔记。
    fn sample_dirs() -> Vec<DirOverview> {
        vec![
            DirOverview {
                name: "idea".into(),
                path: PathBuf::from("C:/notes/idea"),
                subdirs: vec!["drafts".into()],
                notes: vec![NoteInfo {
                    path: PathBuf::from("C:/notes/idea/a.md"),
                    title: "a".into(),
                    tags: vec![],
                }],
            },
            DirOverview {
                name: "ref".into(),
                path: PathBuf::from("C:/notes/ref"),
                subdirs: vec![],
                notes: (0..3)
                    .map(|i| NoteInfo {
                        path: PathBuf::from(format!("C:/notes/ref/r{i}.md")),
                        title: format!("r{i}"),
                        tags: vec![],
                    })
                    .collect(),
            },
        ]
    }

    /// 输入框严格居中，尺寸符合参数。
    #[test]
    fn input_box_is_centered() {
        let (input, _) = layout(1920, 1080, &sample_dirs(), &LayoutParams::default());
        assert_eq!(input.x, (1920 - 420) / 2);
        assert_eq!(input.y, (1080 - 34) / 2);
        assert_eq!(input.w, 420);
        assert_eq!(input.h, 34);
    }

    /// 卡片数量与目录一致；卡片不越出屏幕边界；行顺序 = 头 + 子目录 + 文件。
    #[test]
    fn cards_match_dirs_and_stay_on_screen() {
        let (_, cards) = layout(1920, 1080, &sample_dirs(), &LayoutParams::default());
        assert_eq!(cards.len(), 2);
        for c in &cards {
            assert!(c.rect.x >= 0 && c.rect.x + c.rect.w <= 1920);
            assert!(c.rect.y >= 0 && c.rect.y + c.rect.h <= 1080);
            assert_eq!(c.rows.len(), c.row_rects.len()); // 未滚动时可见行 = 全部行
        }
        assert_eq!(cards[0].rows.len(), 3); // 头 + 子目录 + 1 条
        assert_eq!(cards[0].rows[1], CardRow::SubDir { name: "drafts".into() });
        assert_eq!(cards[1].rows.len(), 4); // 头 + 3 条
        assert!(!cards[1].scrollable()); // 4 行 ≤ 12 上限
    }

    /// 命中测试：点目录头 / 子目录 / 文件行命中对应行，点空白不命中。
    #[test]
    fn hit_test_finds_rows_and_misses_blank() {
        let (_, cards) = layout(1920, 1080, &sample_dirs(), &LayoutParams::default());
        let c0 = &cards[0];
        let hit = hit_test(c0.row_rects[0].x + 5, c0.row_rects[0].y + 5, &cards).unwrap();
        assert_eq!(hit, Hit { card: 0, row: 0 });
        assert_eq!(c0.row_of(hit.row), 0); // 目录头

        // 子目录行
        let hit = hit_test(c0.row_rects[1].x + 5, c0.row_rects[1].y + 5, &cards).unwrap();
        assert_eq!(c0.row_of(hit.row), 1);
        assert_eq!(cards[hit.card].rows[c0.row_of(hit.row)], CardRow::SubDir { name: "drafts".into() });

        // 文件行
        let c1 = &cards[1];
        let hit = hit_test(c1.row_rects[3].x + 5, c1.row_rects[3].y + 5, &cards).unwrap();
        assert_eq!(c1.row_of(hit.row), 3);

        // 屏幕角落空白处
        assert!(hit_test(2, 2, &cards).is_none());
    }

    /// 滚动：行数超过上限时可滚动，滚动后可见行窗口平移、scrollbar 出现。
    #[test]
    fn scroll_card_walks_through_rows() {
        let dirs = vec![DirOverview {
            name: "big".into(),
            path: PathBuf::from("C:/notes/big"),
            subdirs: vec![],
            notes: (0..20)
                .map(|i| NoteInfo {
                    path: PathBuf::from(format!("C:/notes/big/n{i}.md")),
                    title: format!("n{i}"),
                    tags: vec![],
                })
                .collect(),
        }];
        let params = LayoutParams::default();
        let (_, mut cards) = layout(1920, 1080, &dirs, &params);
        let card = &mut cards[0];
        assert!(card.scrollable());
        assert_eq!(card.rows.len(), 21); // 头 + 20 条
        assert_eq!(card.row_rects.len(), params.max_rows); // 只显示可见行

        // 滚动 5 行：可见窗口下移，行矩形位置不变、内容窗口变化
        assert!(scroll_card(card, 5, &params));
        assert_eq!(card.scroll, 5);
        // 底部可见行 = 真实行 5 + 11
        assert_eq!(card.row_of(card.row_rects.len() - 1), 5 + params.max_rows - 1);
        // 滚动条存在且滑块在轨道内
        let bar = scrollbar_rect(card, &params).unwrap();
        let thumb = scrollbar_thumb(card, &params).unwrap();
        assert!(thumb.y >= bar.y && thumb.y + thumb.h <= bar.y + bar.h);
        // 滚动条起点在标题分隔线（padding + header_h）之下
        assert_eq!(bar.y, card.rect.y + params.padding + params.header_h);
        // 加号按钮：滚回顶部后出现在标题带右缘内侧
        assert!(scroll_card(card, -9999, &params));
        let plus = title_plus_rect(card, &params).unwrap();
        assert!(plus.x + plus.w <= card.rect.x + card.rect.w);
        // 加号垂直居中于"边框顶 → 分隔线"区间：不超出卡片顶、不越出标题带
        assert!(plus.y >= card.rect.y);
        assert!(plus.y + plus.h <= card.rect.y + params.padding + params.header_h);
        // 滚动后加号仍然可见且位置不变（固定在卡片右上角）
        assert!(scroll_card(card, 1, &params));
        let plus2 = title_plus_rect(card, &params).unwrap();
        assert_eq!(plus, plus2);

        // 滚到底再滚：不再变化
        assert!(scroll_card(card, 9999, &params));
        assert!(!scroll_card(card, 9999, &params));
        assert_eq!(card.scroll, card.max_scroll());
    }

    /// skatch 卡片：段落 → 每段一 File 行（首行）、宽度加宽、不夹出屏幕。
    #[test]
    fn skatch_card_built() {
        let params = LayoutParams::default();
        let segs = vec!["- 第一条".into(), "## 小节\n- 第二条内容\n- 续行".into()];
        let card = build_skatch_card("C:/notes/skatch.md", &segs, 1920, 1080, &params);
        assert!(card.skatch);
        assert!(!card.temp);
        assert_eq!(card.rect.w, params.card_w + SKATCH_EXTRA_W);
        assert_eq!(card.rows.len(), 3); // 头 + 2 段
        assert_eq!(card.title, "skatch");
        match &card.rows[2] {
            CardRow::File { title, path } => {
                assert_eq!(title, "## 小节");
                assert_eq!(path, "C:/notes/skatch.md");
            }
            _ => panic!("段落应为 File 行"),
        }
        assert_eq!(card.row_rects.len(), 3);
        assert!(card.rect.x >= 0 && card.rect.y >= 0);
    }

    /// 临时子卡片：build_temp_card 标记 temp、夹紧屏幕、可拖动。
    #[test]
    fn temp_card_built_and_clamped() {
        let ov = DirOverview {
            name: "drafts".into(),
            path: PathBuf::from("C:/notes/idea/drafts"),
            subdirs: vec![],
            notes: vec![NoteInfo {
                path: PathBuf::from("C:/notes/idea/drafts/x.md"),
                title: "x".into(),
                tags: vec![],
            }],
        };
        let params = LayoutParams::default();
        let card = build_temp_card(&ov, (99999, 99999), 1920, 1080, &params);
        assert!(card.temp);
        assert_eq!(card.title, "drafts");
        assert!(card.rect.x + card.rect.w <= 1920);
        assert!(card.rect.y + card.rect.h <= 1080);
        assert_eq!(card.row_rects.len(), 2); // 头 + 1 条
    }

    /// 平移：卡片与可见行矩形同步移动，且不越出屏幕。
    #[test]
    fn translate_moves_card_and_rows_with_clamp() {
        let (_, mut cards) = layout(1920, 1080, &sample_dirs(), &LayoutParams::default());
        let mut card = cards.remove(0);
        let old_rect = card.rect;
        let old_rows = card.row_rects.clone();

        translate_card(&mut card, 100, -50, 1920, 1080);
        assert_eq!(card.rect.x, old_rect.x + 100);
        assert_eq!(card.rect.y, old_rect.y - 50);
        for (a, b) in card.row_rects.iter().zip(old_rows.iter()) {
            assert_eq!(a.x - b.x, 100);
            assert_eq!(a.y - b.y, -50);
        }

        translate_card(&mut card, -99999, -99999, 1920, 1080);
        assert!(card.rect.x >= 0 && card.rect.y >= 0);
        translate_card(&mut card, 99999, 99999, 1920, 1080);
        assert!(card.rect.x + card.rect.w <= 1920);
        assert!(card.rect.y + card.rect.h <= 1080);
    }
}
