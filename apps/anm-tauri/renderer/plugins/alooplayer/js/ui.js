// ui.js — 纯渲染辅助: HTML 模板 + 策略栏 + 时间标尺等

export const esc = (s) =>
  String(s ?? '').replace(/[&<>"']/g, (m) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[m]));

const COLORS = ['#4f8cff', '#2ea043', '#d29922', '#f85149', '#bc8cff', '#39c5cf', '#ff7b72', '#58a6ff', '#3fb950', '#e3b341'];

export function colorOf(clip) {
  let h = 0;
  for (const ch of String(clip.id)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return COLORS[h % COLORS.length];
}

export function fmtTime(sec) {
  if (!Number.isFinite(sec) || sec < 0) return '0:00';
  const s = Math.floor(sec % 60);
  const m = Math.floor(sec / 60);
  return `${m}:${String(s).padStart(2, '0')}`;
}

export const STRATEGY_HINTS = {
  loop: '当前片段播完后无缝循环自身；播放中点击右侧列表的片段可临时指定下一段（绿框）。',
  sequential: '自动选择可无缝接续的片段（按列表顺序取第一个）；无可接续时循环当前片段。播放中点击列表片段可临时指定下一段。',
  random: '自动从「起始编号 = 当前结束编号」的可接续片段中随机挑选下一个（绿框指示）；无可接续时循环当前。',
  manual: '点击右侧列表中的片段指定下一段（黄框 = 可无缝接续，点选后变绿）；未选择则播完等待选择。',
};

export function renderStrategyBar(root, strategy) {
  root.querySelectorAll('.strategy-btn').forEach((b) => b.classList.toggle('active', b.dataset.strategy === strategy));
}

// ---------- 片段卡片列表(行式) ----------
// 起始/结束"编号"只是连接标签: 谁的结束编号 = 谁的起始编号, 谁就能无缝接续
// 状态指示:
//   playing     正在播放 → 缩略图上显示 ▶ 播放图标
//   connectable 可无缝接续 → 绿框, 且置顶
//   selected    即将播放(手动点选或自动选择) → 缩略图上显示 ⏳ 等待图标
export function renderClips(root, clips, state = {}) {
  const { currentId, nextId } = state;
  const cur = clips.find((c) => c.id === currentId);
  if (!clips.length) { root.innerHTML = ''; return; }
  root.innerHTML = clips.map((c) => {
    const playing = c.id === currentId;
    // 可接续含自己: 起始编号 = 当前结束编号(自接续时当前片段也绿框置顶)
    const connectable = !!cur && c.startFrame === cur.endFrame;
    const selected = !!nextId && c.id === nextId && !playing;
    const cls = ['clip-card', playing ? 'playing' : '', connectable ? 'connectable' : '', selected ? 'selected' : '']
      .filter(Boolean).join(' ');
    const dur = Number.isFinite(c.duration) && c.duration > 0 ? fmtTime(c.duration) : '';
    const thumb = c.thumb || 'data:image/gif;base64,R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==';
    return `
    <div class="${cls}" data-id="${esc(c.id)}">
      <span class="thumb-wrap">
        <img class="clip-thumb" src="${thumb}" alt="" title="点击预览">
        <span class="thumb-badge"></span>
      </span>
      <div class="clip-info">
        <div class="clip-row1">
          <input class="clip-name" data-field="name" value="${esc(c.name)}" title="点击修改名称" spellcheck="false">
          <span class="clip-dur">${dur}</span>
        </div>
        <div class="clip-row2">
          <label>起<input type="text" inputmode="numeric" pattern="[0-9]*" autocomplete="off" class="num" data-field="start" value="${c.startFrame}"></label>
          <label>止<input type="text" inputmode="numeric" pattern="[0-9]*" autocomplete="off" class="num" data-field="end" value="${c.endFrame}"></label>
        </div>
      </div>
      <button class="btn mini danger clip-del" data-act="del" title="删除">✕</button>
    </div>`;
  }).join('');
}

// ---------- 播完等待选择浮层 ----------
// 可接续含自己: 自接续的片段(起始编号 = 结束编号)也出现在列表里, 点击即循环
export function needItemsHTML(clip, clips) {
  const connects = clips.filter((c) => c.startFrame === clip.endFrame);
  const others = clips.filter((c) => c.startFrame !== clip.endFrame);
  const btn = (c, ok) => `<button class="manual-item" data-need="${esc(c.id)}">
    <span class="clip-dot" style="background:${colorOf(c)}"></span>
    <span class="m-name">${esc(c.name)}</span>
    <span class="dim">编号 ${c.startFrame}–${c.endFrame}</span>
    ${ok ? '<span class="tag ok">无缝</span>' : '<span class="tag warn">硬切</span>'}
  </button>`;
  const groups = [];
  groups.push(`<div class="manual-group">✦ 无缝接续（起始编号 = ${clip.endFrame}）</div>`);
  groups.push(connects.length ? connects.map((c) => btn(c, true)).join('') : '<div class="dim" style="padding:2px 0">没有可无缝接续的片段</div>');
  if (others.length) {
    groups.push('<div class="manual-group">其他片段（硬切）</div>');
    groups.push(others.map((c) => btn(c, false)).join(''));
  }
  return groups.join('');
}

// ---------- 时间标尺 ----------
export function renderRulerTicks(root, duration) {
  const dur = Number.isFinite(duration) && duration > 0 ? duration : 0;
  let ticks = '';
  if (dur <= 0) {
    ticks = '';
  } else {
    // 目标 ~10 个刻度, 取 1/2/5/10/15/30/60 的整数秒步长
    const rawStep = dur / 10;
    const candidates = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600];
    const step = candidates.find((c) => c >= rawStep) || 600;
    for (let t = 0; t <= dur + 1e-6; t += step) {
      const pct = ((t / dur) * 100).toFixed(3);
      ticks += `<div class="tick" style="left:${pct}%"><span class="tick-label">${fmtTime(t)}</span></div>`;
    }
  }
  root.innerHTML = ticks;
  const marker = document.getElementById('ruler-marker');
  if (marker) marker.style.left = '0%';
}

export function updateRulerMarker(duration, t) {
  const marker = document.getElementById('ruler-marker');
  if (!marker) return;
  const dur = Number.isFinite(duration) && duration > 0 ? duration : 0;
  const pct = dur <= 0 ? 0 : (Math.min(dur, Math.max(0, t)) / dur) * 100;
  marker.style.left = `${Math.min(100, Math.max(0, pct)).toFixed(2)}%`;
}
