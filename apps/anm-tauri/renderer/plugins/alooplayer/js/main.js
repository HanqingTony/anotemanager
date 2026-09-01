// main.js — 应用入口: 装配播放器、片段管理、策略控制与预览
// 模型: 片段 = 视频文件 + 起始编号/结束编号(纯连接标签, 不映射媒体帧)
import { SeamlessPlayer } from './player.js';
import { probeVideo } from './fps.js';
import * as store from './store.js';
import {
  esc, colorOf, fmtTime, STRATEGY_HINTS, renderStrategyBar, renderClips,
  needItemsHTML, renderRulerTicks, updateRulerMarker,
} from './ui.js';

const $ = (s) => document.querySelector(s);

const els = {
  strategyBar: $('#strategy-bar'), strategyHint: $('#strategy-hint'),
  stage: $('#stage'),
  overlayNeed: $('#overlay-need'), needTitle: $('#need-title'), needSub: $('#need-sub'),
  needList: $('#need-list'), needLoop: $('#need-loop'), needStop: $('#need-stop'),
  overlayStopped: $('#overlay-stopped'), btnBigPlay: $('#btn-big-play'), stoppedHint: $('#stopped-hint'),
  btnPlay: $('#btn-play'), btnStop: $('#btn-stop'),
  curBadge: $('#cur-badge'), frameInfo: $('#frame-info'), statusLine: $('#status-line'),
  seek: $('#seek'), rulerTicks: $('#ruler-ticks'), ruler: $('#ruler'),
  nextRow: $('#next-row'),
  btnDemo: $('#btn-demo'), btnAdd: $('#btn-add'), fileInput: $('#file-input'),
  dropZone: $('#drop-zone'), clipList: $('#clip-list'), emptyHint: $('#empty-hint'),
  previewModal: $('#preview-modal'), pvTitle: $('#pv-title'), pvVideo: $('#pv-video'),
  pvFrame: $('#pv-frame'), pvCaptures: $('#pv-captures'),
  pvFirst: $('#pv-first'), pvPrev: $('#pv-prev'), pvNext: $('#pv-next'),
  pvPlay: $('#pv-play'), pvCapture: $('#pv-capture'), pvClose: $('#pv-close'),
};

let clips = [];
let lastPlayedId = null;
let seekDragging = false;
let statusTimer = null;
let pv = null; // 预览状态 { clip, fps, t, dur }

const STRATEGY_KEY = 'alooplayer-strategy';
const STRATEGY_NAMES = { loop: '🔁 循环当前', sequential: '➡️ 顺序接续', random: '🎲 随机接续', manual: '👆 手动接续' };
let strategy = localStorage.getItem(STRATEGY_KEY) || 'random';

const player = new SeamlessPlayer(els.stage, {
  onTime: updateTimeUI,
  onClipChange,
  onState: onStateChange,
  onStatus,
  onNeedSelect,
});

// ---------- 工具 ----------

function status(msg, isErr = false) {
  els.statusLine.textContent = msg;
  els.statusLine.classList.toggle('err', isErr);
  clearTimeout(statusTimer);
  if (!isErr) statusTimer = setTimeout(() => { els.statusLine.textContent = ''; }, 5000);
}

function persist(clip) {
  store.saveClip(clip).catch((e) => { console.error('保存失败', e); status('保存失败: ' + e.message, true); });
}

function playClipId(id) {
  const clip = clips.find((c) => c.id === id);
  if (!clip) return;
  lastPlayedId = id;
  player.playClip(clip);
}

// ---------- 片段列表渲染(带当前/下一个状态) ----------

function refreshClipList() {
  renderClips(els.clipList, clips, {
    currentId: player.currentClip?.id,
    nextId: player.nextClip?.id,
  });
  clips.forEach(ensureThumb);
}

// 为片段生成小缩略图(顺带记录时长); 生成完就地更新卡片
function ensureThumb(clip) {
  if (clip.thumb || clip._thumbBusy) return;
  clip._thumbBusy = true;
  const v = document.createElement('video');
  v.muted = true;
  v.playsInline = true;
  v.preload = 'metadata';
  v.src = clip.url;
  v.style.cssText = 'position:fixed;left:-9999px;top:0;width:4px;height:4px;opacity:0;pointer-events:none;';
  document.body.appendChild(v);
  v.addEventListener('loadedmetadata', () => {
    clip.duration = Number.isFinite(v.duration) ? v.duration : null;
    try { v.currentTime = Math.min(0.1, (v.duration || 0) * 0.1); } catch (e) { /* ignore */ }
  });
  v.addEventListener('seeked', () => {
    try {
      const c = document.createElement('canvas');
      c.width = 160;
      c.height = 90;
      c.getContext('2d').drawImage(v, 0, 0, 160, 90);
      clip.thumb = c.toDataURL('image/jpeg', 0.6);
      const card = document.querySelector(`.clip-card[data-id="${CSS.escape(clip.id)}"]`);
      const img = card && card.querySelector('.clip-thumb');
      if (img) img.src = clip.thumb;
      const dur = card && card.querySelector('.clip-dur');
      if (dur && clip.duration) dur.textContent = fmtTime(clip.duration);
    } catch (e) { /* ignore */ }
    v.removeAttribute('src');
    v.load();
    v.remove();
  });
  v.addEventListener('error', () => { try { v.remove(); } catch (e) { /* ignore */ } });
}

// ---------- 播放器回调 ----------

function updateTimeUI(clip, t, duration) {
  const dur = Number.isFinite(duration) && duration > 0 ? duration : 0;
  els.frameInfo.textContent = `${fmtTime(t)} / ${fmtTime(dur)}`;
  if (!seekDragging) els.seek.value = t;
  updateRulerMarker(dur, t);
}

function onClipChange(clip, duration) {
  const color = colorOf(clip);
  const dur = Number.isFinite(duration) && duration > 0 ? duration : 0;
  els.curBadge.classList.remove('hidden');
  els.curBadge.innerHTML = `<span class="dot" style="background:${color}"></span>${esc(clip.name)} <span class="dim">编号 ${clip.startFrame}–${clip.endFrame}</span> <span class="tag ok">${STRATEGY_NAMES[strategy] || ''}</span>`;
  els.seek.min = 0;
  els.seek.max = dur > 0 ? dur : 1;
  els.seek.step = 'any'; // 允许亚秒进度(短片段只有 0.1s)
  renderRulerTicks(els.rulerTicks, dur);
  highlightCurrentCard();
  refreshClipList();
  updateNextRow();
}

function onStateChange(s) {
  const playing = s === 'playing';
  els.btnPlay.textContent = playing ? '⏸' : '▶';
  els.btnStop.disabled = !playing && s !== 'paused';
  els.seek.disabled = !(s === 'playing' || s === 'paused');
  els.overlayNeed.classList.toggle('hidden', s !== 'need-select');
  const stopped = s === 'stopped' || s === 'idle';
  els.overlayStopped.classList.toggle('hidden', !stopped);
  if (s === 'loading' && player.currentClip) {
    els.frameInfo.textContent = '0:00 / 0:00';
  }
  if (stopped) {
    els.stoppedHint.textContent = clips.length ? '选择片段或点击「▶ 播放」开始' : '先添加视频片段，或点击「🎬 加载演示片段」';
    els.frameInfo.textContent = '—';
    els.curBadge.classList.add('hidden');
  }
  refreshClipList();
  updateNextRow();
}

function onNeedSelect(clip) {
  els.needTitle.textContent = `「${clip.name}」已播放完（结束编号 ${clip.endFrame}）`;
  const connects = player.connectableClips(clip);
  els.needSub.textContent = connects.length
    ? '选择下一段即可无缝续播（起始编号 = 当前结束编号）：'
    : '没有可无缝接续的片段，请选择其他片段或循环当前：';
  els.needList.innerHTML = needItemsHTML(clip, clips);
  els.overlayNeed.classList.remove('hidden');
}

function onStatus(s) {
  if (s.kind === 'error') { status(s.message, true); return; }
  if (s.kind === 'timeout') { status('加载超时，请检查片段文件', true); return; }
  if (s.kind === 'switch') status(`切换: ${s.from?.name} → ${s.to?.name}`);
  if (s.kind === 'next') {
    refreshClipList(); // 更新"下一个"绿框
    updateNextRow();
  }
}

// ---------- 下一段信息 / 手动面板 ----------

function updateNextRow() {
  const next = player.nextClip;
  const cur = player.currentClip;
  if (!cur || player.state === 'stopped' || player.state === 'idle') { els.nextRow.innerHTML = ''; return; }
  if (player.strategy === 'manual') {
    els.nextRow.innerHTML = player.manualNext
      ? `已选择下一段: <b>${esc(player.manualNext.name)}</b> <span class="tag ${player.manualNext.startFrame === cur.endFrame ? 'ok' : 'warn'}">${player.manualNext.startFrame === cur.endFrame ? '无缝' : '硬切'}</span> <span class="dim">编号 ${player.manualNext.startFrame}–${player.manualNext.endFrame}</span>`
      : '<span class="dim">手动模式：点击右侧列表中想接续的片段（黄框为可无缝接续）；未选择则播完等待</span>';
    return;
  }
  if (player.manualNext) {
    // 手动点选覆盖了自动策略
    els.nextRow.innerHTML = `已选择下一段: <b>${esc(player.manualNext.name)}</b> <span class="tag ${player.manualNext.startFrame === cur.endFrame ? 'ok' : 'warn'}">${player.manualNext.startFrame === cur.endFrame ? '无缝' : '硬切'}</span> <span class="dim">编号 ${player.manualNext.startFrame}–${player.manualNext.endFrame}</span>`;
    return;
  }
  if (!next) { els.nextRow.innerHTML = ''; return; }
  let tag = '硬切';
  let cls = 'warn';
  if (next.id === cur.id) { tag = '循环当前'; cls = 'ok'; }
  else if (next.startFrame === cur.endFrame) { tag = '无缝衔接'; cls = 'ok'; }
  if (player.strategy === 'random' && next.id !== cur.id) {
    const candidates = player.connectableClips(cur).map((c) => esc(c.name)).join('、');
    els.nextRow.innerHTML = `🎲 随机接续：本段播完后将从 <b>${candidates}</b> 中随机选择`;
    return;
  }
  els.nextRow.innerHTML = `下一段: <b>${esc(next.name)}</b> <span class="tag ${cls}">${tag}</span> <span class="dim">编号 ${next.startFrame}–${next.endFrame}</span>`;
}

function highlightCurrentCard() {
  els.clipList.querySelectorAll('.clip-card').forEach((card) => {
    card.classList.toggle('playing', card.dataset.id === player.currentClip?.id);
  });
}

// ---------- 片段管理 ----------

function normalizeClip(c) {
  c.startFrame = Math.max(1, Math.round(c.startFrame) || 1);
  c.endFrame = Math.max(c.startFrame, Math.round(c.endFrame) || c.startFrame);
  c.name = String(c.name || '').trim() || c.fileName || '未命名';
}

function editClip(id, patch) {
  const c = clips.find((x) => x.id === id);
  if (!c) return;
  Object.assign(c, patch);
  normalizeClip(c);
  refreshClipList();
  player.setClips(clips);
  persist(c);
  updateNextRow();
  // 编号变化影响连接关系; 若改的是当前片段, 从头重播
  if (player.currentClip?.id === id && (patch.startFrame !== undefined || patch.endFrame !== undefined)) {
    player.restartCurrent();
  }
}

async function deleteClip(id) {
  const c = clips.find((x) => x.id === id);
  if (!c) return;
  if (!confirm(`删除片段「${c.name}」？`)) return;
  clips = clips.filter((x) => x.id !== id);
  URL.revokeObjectURL(c.url);
  await store.removeClip(id).catch((e) => console.error(e));
  if (player.currentClip?.id === id) player.stop();
  if (player.manualNext?.id === id) player.chooseNext(null);
  player.setClips(clips);
  refreshClipList();
  updateEmptyHint();
  updateNextRow();
  status(`已删除「${c.name}」`);
}

async function addFiles(fileList) {
  const files = [...fileList].filter(
    (f) => f.type.startsWith('video/') || /\.(mp4|webm|mov|m4v|mkv|ogv)$/i.test(f.name),
  );
  if (!files.length) { status('未找到视频文件', true); return; }
  for (let i = 0; i < files.length; i++) {
    const f = files[i];
    status(`正在添加 ${f.name}（${i + 1}/${files.length}）…`);
    const id = crypto.randomUUID ? crypto.randomUUID() : 'c' + Date.now() + Math.random().toString(36).slice(2);
    const clip = {
      id,
      name: f.name.replace(/\.[^.]+$/, ''),
      fileName: f.name,
      blob: f,
      url: URL.createObjectURL(f),
      startFrame: 1,
      endFrame: 1,
      createdAt: Date.now(),
    };
    clips.push(clip);
    await store.saveClip(clip).catch((e) => console.error(e));
    refreshClipList();
    player.setClips(clips);
    updateEmptyHint();
  }
  status(files.length > 1 ? `已添加 ${files.length} 个片段` : `已添加「${files[0].name}」`);
  onStateChange(player.state);
}

async function loadDemo() {
  status('正在加载演示片段…');
  try {
    const res = await fetch('demo/manifest.json');
    if (!res.ok) throw new Error('HTTP ' + res.status);
    const manifest = await res.json();
    for (const item of manifest.clips) {
      const fr = await fetch('demo/clips/' + item.file);
      if (!fr.ok) throw new Error('缺少 ' + item.file);
      const blob = await fr.blob();
      const id = crypto.randomUUID ? crypto.randomUUID() : 'd' + Date.now() + Math.random().toString(36).slice(2);
      const clip = {
        id,
        name: item.name,
        fileName: item.file,
        blob,
        url: URL.createObjectURL(blob),
        startFrame: item.startFrame,
        endFrame: item.endFrame,
        createdAt: Date.now(),
      };
      clips.push(clip);
      await store.saveClip(clip).catch((e) => console.error(e));
    }
    player.setClips(clips);
    refreshClipList();
    updateEmptyHint();
    onStateChange(player.state);
    status(`已加载演示片段链: ${manifest.clips.map((c) => c.name).join(' → ')}`);
  } catch (e) {
    console.error(e);
    status('加载演示片段失败（请确认通过 HTTP 服务访问）: ' + e.message, true);
  }
}

function updateEmptyHint() {
  els.emptyHint.classList.toggle('hidden', clips.length > 0);
}

// ---------- 预览(内部自动检测帧率用于逐帧步进) ----------

function openPreview(clip) {
  pv = { clip, fps: null, t: 0, dur: 0 };
  els.pvTitle.textContent = `${clip.name} · ${clip.fileName}`;
  els.pvCaptures.innerHTML = '';
  els.pvVideo.src = clip.url;
  els.previewModal.classList.remove('hidden');
  els.pvVideo.load();
  els.pvVideo.addEventListener('loadeddata', () => {
    els.pvVideo.currentTime = 0;
    pv.dur = els.pvVideo.duration || 0;
    pv.t = 0;
    updatePvFrame();
  }, { once: true });
  // 内部检测帧率(用于"上一帧/下一帧"步进), 用户无需关心
  probeVideo(clip.blob, { maxFrames: 20, timeoutMs: 3000 }).then((fps) => {
    if (pv && pv.clip.id === clip.id) pv.fps = fps || 30;
  });
}

function closePreview() {
  els.previewModal.classList.add('hidden');
  els.pvVideo.pause();
  els.pvVideo.removeAttribute('src');
  els.pvVideo.load();
  pv = null;
}

function updatePvFrame() {
  if (pv) {
    // 预览用秒级精度显示(演示片段可能只有 0.1s)
    els.pvFrame.textContent = `${pv.t.toFixed(2)}s / ${pv.dur.toFixed(2)}s`;
  }
}

function watchPvFrame() {
  if ('requestVideoFrameCallback' in els.pvVideo) {
    const loop = () => {
      els.pvVideo.requestVideoFrameCallback((now, meta) => {
        if (pv) {
          pv.t = Math.min(pv.dur, Math.max(0, meta.mediaTime));
          updatePvFrame();
        }
        loop();
      });
    };
    loop();
  } else {
    els.pvVideo.addEventListener('timeupdate', () => {
      if (pv) { pv.t = Math.min(pv.dur, Math.max(0, els.pvVideo.currentTime)); updatePvFrame(); }
    });
  }
}

function pvStep(delta) {
  if (!pv || !pv.fps) return;
  const step = 1 / pv.fps;
  pv.t = Math.min(pv.dur, Math.max(0, pv.t + delta * step));
  els.pvVideo.currentTime = pv.t;
  updatePvFrame();
}

function pvCapture() {
  if (!pv) return;
  const canvas = document.createElement('canvas');
  canvas.width = 640;
  canvas.height = 360;
  const ctx = canvas.getContext('2d');
  ctx.drawImage(els.pvVideo, 0, 0, 640, 360);
  const url = canvas.toDataURL('image/jpeg', 0.9);
  const wrap = document.createElement('div');
  wrap.className = 'capture-item';
  const img = document.createElement('img');
  img.src = url;
  const cap = document.createElement('div');
  cap.className = 'capture-cap';
  cap.textContent = `${pv.clip.name} · ${pv.t.toFixed(2)}s`;
  // 下载链接(文件名清洗掉路径非法字符)
  const dl = document.createElement('a');
  dl.className = 'capture-dl';
  dl.href = url;
  dl.download = `${String(pv.clip.name).replace(/[\\/:*?"<>|\s]+/g, '_')}-${pv.t.toFixed(2)}s.jpg`;
  dl.textContent = '⬇ 下载';
  dl.title = '下载这张截帧';
  cap.append(dl);
  wrap.append(img, cap);
  els.pvCaptures.prepend(wrap);
  status(`已截帧: ${cap.textContent}`);
}

// ---------- 事件绑定 ----------

function bindUI() {
  // 策略
  els.strategyBar.addEventListener('click', (e) => {
    const btn = e.target.closest('.strategy-btn');
    if (!btn) return;
    strategy = btn.dataset.strategy;
    localStorage.setItem(STRATEGY_KEY, strategy);
    renderStrategyBar(els.strategyBar, strategy);
    els.strategyHint.textContent = STRATEGY_HINTS[strategy];
    player.setStrategy(strategy);
    // 同步当前徽章上的策略标签
    const tag = els.curBadge.querySelector('.tag');
    if (tag) tag.textContent = STRATEGY_NAMES[strategy] || '';
    refreshClipList();
    updateNextRow();
  });
  renderStrategyBar(els.strategyBar, strategy);
  els.strategyHint.textContent = STRATEGY_HINTS[strategy];

  // 传输控制
  els.btnPlay.addEventListener('click', () => {
    if (player.state === 'stopped' || player.state === 'idle') {
      const clip = clips.find((c) => c.id === lastPlayedId) || clips[0];
      if (clip) playClipId(clip.id);
    } else {
      player.toggle();
    }
  });
  els.btnBigPlay.addEventListener('click', () => {
    const clip = clips.find((c) => c.id === lastPlayedId) || clips[0];
    if (clip) playClipId(clip.id);
  });
  els.btnStop.addEventListener('click', () => player.stop());

  // 进度条与标尺(时间)
  els.seek.addEventListener('input', () => { seekDragging = true; player.seekToTime(parseFloat(els.seek.value) || 0); });
  els.seek.addEventListener('change', () => { seekDragging = false; });
  els.seek.addEventListener('pointerdown', () => { seekDragging = true; });
  els.seek.addEventListener('pointerup', () => { seekDragging = false; });
  els.ruler.addEventListener('click', (e) => {
    const clip = player.currentClip;
    if (!clip) return;
    const rect = els.ruler.getBoundingClientRect();
    const pct = Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width));
    const dur = els.seek.max > 1 ? els.seek.max : 0;
    player.seekToTime(pct * dur);
  });

  // 片段列表: 点击卡片 = 播放(未播放时) 或 指定/取消下一段(播放中); 缩略图 = 预览
  els.clipList.addEventListener('click', (e) => {
    const thumb = e.target.closest('.clip-thumb');
    if (thumb) {
      const card = thumb.closest('.clip-card');
      if (card) openPreview(clips.find((c) => c.id === card.dataset.id));
      return;
    }
    const del = e.target.closest('[data-act="del"]');
    if (del) {
      const card = del.closest('.clip-card');
      if (card) deleteClip(card.dataset.id);
      return;
    }
    const card = e.target.closest('.clip-card');
    if (!card) return;
    const clip = clips.find((c) => c.id === card.dataset.id);
    if (!clip) return;
    if (player.state === 'stopped' || player.state === 'idle') {
      playClipId(clip.id);
      return;
    }
    // 播放中: 点击 = 指定/取消下一段(任意策略下都优先于自动选择)
    const isSelected = player.manualNext?.id === clip.id;
    player.chooseNext(isSelected ? null : clip);
    status(isSelected ? `已取消选择「${clip.name}」` : `已选择下一段: ${clip.name}`);
    refreshClipList();
    updateNextRow();
  });

  // 播完等待浮层
  els.needList.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-need]');
    if (!btn) return;
    player.chooseNext(clips.find((c) => c.id === btn.dataset.need) || null);
  });
  els.needLoop.addEventListener('click', () => {
    // 「循环当前」= 切换到循环策略并立即续播(否则手动模式下每轮都要重新选择)
    strategy = 'loop';
    localStorage.setItem(STRATEGY_KEY, strategy);
    renderStrategyBar(els.strategyBar, strategy);
    els.strategyHint.textContent = STRATEGY_HINTS[strategy];
    player.setStrategy('loop');
    status('已切换为「循环当前」策略');
  });
  els.needStop.addEventListener('click', () => player.stop());

  // 添加 / 删除 / 编辑片段
  els.btnAdd.addEventListener('click', () => els.fileInput.click());
  els.fileInput.addEventListener('change', () => { addFiles(els.fileInput.files); els.fileInput.value = ''; });
  els.btnDemo.addEventListener('click', loadDemo);

  els.dropZone.addEventListener('dragover', (e) => { e.preventDefault(); els.dropZone.classList.add('drag'); });
  els.dropZone.addEventListener('dragleave', () => els.dropZone.classList.remove('drag'));
  els.dropZone.addEventListener('drop', (e) => {
    e.preventDefault();
    els.dropZone.classList.remove('drag');
    addFiles(e.dataTransfer.files);
  });
  els.stage.addEventListener('dragover', (e) => e.preventDefault());
  els.stage.addEventListener('drop', (e) => { e.preventDefault(); addFiles(e.dataTransfer.files); });
  window.addEventListener('dragover', (e) => e.preventDefault());
  window.addEventListener('drop', (e) => e.preventDefault());

  els.clipList.addEventListener('input', (e) => {
    // 编号输入框只接受正整数(过滤非数字字符)
    const input = e.target.closest('.num');
    if (!input) return;
    const clean = input.value.replace(/\D/g, '');
    if (clean !== input.value) input.value = clean;
  });

  els.clipList.addEventListener('change', (e) => {
    const input = e.target.closest('.clip-name, .num');
    if (!input) return;
    const card = input.closest('.clip-card');
    if (!card) return;
    const id = card.dataset.id;
    const field = input.dataset.field;
    if (field === 'name') {
      const name = input.value.trim();
      if (!name) { refreshClipList(); return; }
      editClip(id, { name });
    } else if (field === 'start') {
      editClip(id, { startFrame: parseInt(input.value, 10) });
    } else if (field === 'end') {
      editClip(id, { endFrame: parseInt(input.value, 10) });
    }
  });

  // 预览弹窗
  els.pvClose.addEventListener('click', closePreview);
  els.pvFirst.addEventListener('click', () => { if (pv) { pv.t = 0; els.pvVideo.currentTime = 0; updatePvFrame(); } });
  els.pvPrev.addEventListener('click', () => pvStep(-1));
  els.pvNext.addEventListener('click', () => pvStep(1));
  els.pvPlay.addEventListener('click', () => {
    if (els.pvVideo.paused) els.pvVideo.play().catch(() => {});
    else els.pvVideo.pause();
  });
  els.pvCapture.addEventListener('click', pvCapture);
  els.previewModal.addEventListener('click', (e) => { if (e.target === els.previewModal) closePreview(); });
  watchPvFrame();

  // 键盘: 空格播放/暂停
  window.addEventListener('keydown', (e) => {
    if (e.code !== 'Space') return;
    const t = e.target;
    if (t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || t instanceof HTMLButtonElement) return;
    if (!els.previewModal.classList.contains('hidden')) return;
    e.preventDefault();
    player.toggle();
  });
}

// ---------- 启动 ----------

async function init() {
  bindUI();
  const saved = await store.loadClips().catch((e) => { console.error('读取存储失败', e); return []; });
  clips = saved;
  for (const c of clips) {
    if (c.blob) c.url = URL.createObjectURL(c.blob);
  }
  player.setClips(clips);
  refreshClipList();
  updateEmptyHint();
  onStateChange(player.state);
  // 调试入口
  window.alooplayer = { player, getClips: () => clips };
}

init();
