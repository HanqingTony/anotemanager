// player.js — SeamlessPlayer: 双缓冲无缝连续播放引擎
//
// 设计(双缓冲):
//  1. 两个 <video> 元素: 前台播放, 后台预载。
//  2. 后台预载: preload="auto" + seek 到 0 + 等待 canplaythrough ——
//     整段数据就绪、首帧已解码可立即显示。切换瞬间直接切过去,
//     没有黑屏、没有解码延迟。
//  3. 前台接近末尾(剩余时间 ≤ 提前量)时立即切换:
//     瞬切 opacity(0ms 过渡) + 后台 play() + 角色互换 + 预载下下段。
//     提前量 = min(300ms, 时长/3), 对短片段自动收缩。
//  4. ended 仅作兜底(正常情况下提前切换已在播完前完成)。
//  5. 简单卡顿恢复: 播放中 2s 无 timeupdate(呈现管道停摆)时重启当前片段。

const LEAD_MS = 300;   // 提前切换的最大提前量(毫秒)
const STALL_MS = 2000; // 超过该时长无进度更新视为卡顿

export class SeamlessPlayer {
  constructor(stage, hooks = {}) {
    this.hooks = hooks;
    this.clips = [];
    this.strategy = 'random'; // loop | sequential | random | manual
    this.state = 'idle';      // idle | loading | playing | paused | need-select | stopped
    this.currentClip = null;
    this.nextClip = null;
    this.manualNext = null;   // 手动点选(优先于策略自动选择)
    this.urlFor = hooks.urlFor || ((c) => c.url);

    this.els = [];
    for (let i = 0; i < 2; i++) {
      const v = document.createElement('video');
      v.className = 'player-video';
      v.playsInline = true;
      v.preload = 'auto'; // 后台需要完整缓冲, 切换时才能立即出帧
      v.dataset.ready = '';
      v.dataset.srcUrl = '';
      v.addEventListener('error', () => {
        if (v.__clip) {
          this.hooks.onStatus?.({ kind: 'error', message: `片段「${v.__clip.name}」加载失败`, error: v.error });
        }
      });
      v.addEventListener('timeupdate', () => {
        v.__lastSeen = performance.now();
        v.__lastT = v.currentTime;
        // 进度上报(播放中每 ~250ms; 切换瞬间另有 onTime(0) 上报)
        if (v === this.cur && this.state === 'playing' && v.__clip) {
          this.hooks.onTime?.(v.__clip, v.currentTime, v.duration || 0);
        }
      });
      v.addEventListener('ended', () => {
        // 兜底: 提前切换失效时, 播完也要切(此时后台应已就绪)
        if (v === this.cur && this.state === 'playing') this._transition();
      });
      stage.appendChild(v);
      this.els.push(v);
    }
    this.cur = this.els[0];
    this.alt = this.els[1];
    this.cur.classList.add('is-active');

    // rAF 循环: 进度监视(提前切换) + 卡顿恢复
    const tick = () => { this._watch(); requestAnimationFrame(tick); };
    requestAnimationFrame(tick);
  }

  // ---------- 对外 API ----------

  setClips(list) { this.clips = list || []; this._refreshNext(); }

  setStrategy(s) {
    if (!['loop', 'sequential', 'random', 'manual'].includes(s)) s = 'random';
    this.strategy = s;
    this.manualNext = null;
    if (this.state === 'need-select') {
      const next = this._decideNext();
      if (next) {
        this.nextClip = next;
        this.hooks.onStatus?.({ kind: 'switch', from: this.currentClip, to: next });
        this._transition();
      } else {
        this.hooks.onNeedSelect?.(this.currentClip);
      }
      return;
    }
    this._refreshNext();
  }

  playClip(clip) {
    if (!clip) return;
    this._stopInternal(false);
    this.currentClip = clip;
    this._setState('loading');
    this._start(this.cur, clip);
    this._whenReady(this.cur, () => {
      if (this.currentClip !== clip || this.state === 'stopped') return;
      this._playCurrent();
      this._setState('playing');
      this.hooks.onClipChange?.(clip, this.cur.duration || 0);
      this._refreshNext();
    });
  }

  // 编号被修改后, 让当前播放回到开头
  restartCurrent() {
    const clip = this.currentClip;
    if (!clip || this.state === 'idle' || this.state === 'stopped') return;
    this._prepare(this.cur, clip);
    this._whenReady(this.cur, () => {
      this._playCurrent();
      this._setState('playing');
      this._refreshNext();
    });
  }

  // 手动选择下一段(播放中/暂停/播完等待均可; null 取消)
  chooseNext(clip) {
    this.manualNext = clip || null;
    if (!clip) { this._refreshNext(); return; }
    if (this.state === 'need-select') {
      this.nextClip = clip;
      this.hooks.onStatus?.({ kind: 'switch', from: this.currentClip, to: clip });
      this._transition();
    } else if (this.state === 'playing' || this.state === 'paused') {
      this._refreshNext();
    }
  }

  pause() {
    if (this.state === 'playing') {
      this.cur.pause();
      this._setState('paused');
    }
  }
  resume() { if (this.state === 'paused') { this.cur.play().catch(() => {}); this._setState('playing'); } }
  toggle() { this.state === 'playing' ? this.pause() : this.resume(); }

  stop() { this._stopInternal(true); }

  seekToTime(t) {
    const el = this.cur;
    const clip = this.currentClip;
    if (!el || !clip) return;
    const dur = el.duration || 0;
    const target = Math.min(Math.max(0, t), dur);
    if (el.readyState >= 1) { try { el.currentTime = target; } catch (e) { /* ignore */ } }
    this.hooks.onTime?.(clip, target, dur);
  }

  connectableClips(clip) {
    // 连接标签: 所有"起始编号 = 本片段结束编号"的片段都可无缝接续,
    // 包括自己(起始编号 = 结束编号时即为自接续, 自然实现循环)
    return this.clips.filter((c) => c.startFrame === clip.endFrame);
  }

  // ---------- 内部 ----------

  _setState(s) { this.state = s; this.hooks.onState?.(s); }

  _stopInternal(clearAll) {
    for (const el of this.els) {
      el.pause();
      el.__clip = null;
      el.__lastSeen = 0;
      el.dataset.ready = '';
      if (clearAll) {
        el.dataset.srcUrl = '';
        try { el.removeAttribute('src'); el.load(); } catch (e) { /* ignore */ }
      }
    }
    this.cur = this.els[0];
    this.alt = this.els[1];
    this.cur.classList.add('is-active');
    this.alt.classList.remove('is-active');
    this.currentClip = null;
    this.nextClip = null;
    this.manualNext = null;
    this._setState('stopped');
  }

  // 让元素播放当前片段: 确保 src 就绪后 seek 0 + play
  _start(el, clip) {
    el.__clip = clip;
    this._prepare(el, clip);
    this._whenReady(el, () => this._playCurrent());
  }

  _playCurrent() {
    const el = this.cur;
    try { el.currentTime = 0; } catch (e) { /* ignore */ }
    el.play().catch((e) => this.hooks.onStatus?.({ kind: 'error', message: `播放失败: ${e?.message || e}`, error: e }));
  }

  // 预载后台: 加载 src → seek 0 → 等 canplaythrough(整段就绪, 首帧可立即显示)
  _prepare(el, clip) {
    el.__clip = clip;
    const url = this.urlFor(clip);
    if (el.dataset.srcUrl !== url) {
      el.dataset.srcUrl = url;
      el.dataset.ready = '';
      el.src = url;
    }
    if (el.dataset.ready === String(clip.id)) return;
    const finish = () => { el.dataset.ready = String(clip.id); };
    const seekAndWait = () => {
      try { el.currentTime = 0; } catch (e) { /* ignore */ }
      if (el.readyState >= 4) { finish(); return; }
      const onReady = () => { el.removeEventListener('canplaythrough', onReady); finish(); };
      el.addEventListener('canplaythrough', onReady, { once: true });
      // 兜底: 5s 内未就绪(如超大文件)也标记完成, 避免永远等
      setTimeout(() => { el.removeEventListener('canplaythrough', onReady); finish(); }, 5000);
    };
    if (el.readyState >= 1) seekAndWait();
    else el.addEventListener('loadedmetadata', seekAndWait, { once: true });
  }

  _whenReady(el, cb, timeoutMs = 10000) {
    const start = Date.now();
    const poll = () => {
      if (el.__clip && el.dataset.ready === String(el.__clip.id)) { cb(); return; }
      if (Date.now() - start > timeoutMs) {
        this.hooks.onStatus?.({ kind: 'error', message: `片段加载超时: ${el.__clip?.name || ''}` });
        return;
      }
      setTimeout(poll, 40);
    };
    poll();
  }

  _decideNext() {
    const cur = this.currentClip;
    if (!cur) return null;
    // 手动点选(点击列表卡片)优先于策略自动选择; 播完一次后清除, 回到策略默认
    if (this.manualNext) return this.manualNext;
    const connects = this.connectableClips(cur);
    switch (this.strategy) {
      case 'loop': return cur;
      case 'sequential': return connects[0] || cur;
      case 'random': return connects.length ? connects[(Math.random() * connects.length) | 0] : cur;
      case 'manual': return null; // 手动模式: 未点选则播完等待选择
      default: return cur;
    }
  }

  _refreshNext() {
    if (this.state === 'idle' || this.state === 'stopped') { this.nextClip = null; return; }
    const next = this._decideNext();
    this.nextClip = next;
    if (next) this._prepare(this.alt, next);
    else this.alt.dataset.ready = '';
    this.hooks.onStatus?.({ kind: 'next', next, strategy: this.strategy });
  }

  // ---------- 进度监视(rAF)与切换 ----------

  _watch() {
    if (this.state !== 'playing') return;
    const front = this.cur;
    const clip = front.__clip;
    if (!clip || front.paused) return;
    // 卡顿恢复: 播放中长时间无 timeupdate(呈现管道停摆)
    const now = performance.now();
    if (front.__lastSeen && now - front.__lastSeen > STALL_MS) {
      this.hooks.onStatus?.({ kind: 'error', message: `播放卡顿, 已自动恢复: ${clip.name}` });
      this._recover();
      return;
    }
    // 提前切换: 剩余时间 ≤ 提前量(与时长相关, 短片段自动收缩)
    const dur = front.duration || 0;
    if (dur <= 0) return;
    const remaining = (dur - front.currentTime) * 1000;
    const lead = Math.min(LEAD_MS, (dur * 1000) / 3);
    if (remaining <= lead) this._transition();
  }

  _recover() {
    for (const el of this.els) el.pause();
    this._playCurrent();
    this._refreshNext();
  }

  _transition() {
    // playing: 常规切换; need-select: 从"播完等待"恢复(用户点选/切换策略后)
    if (this.state !== 'playing' && this.state !== 'need-select') return;
    const front = this.cur;
    if (!front.__clip) return;
    const from = this.currentClip;
    const next = this.nextClip;
    if (!next) {
      // 手动模式且未选择: 停在当前帧, 等待用户选择(无黑屏风险: 前台画面保留)
      front.pause();
      this._setState('need-select');
      this.hooks.onNeedSelect?.(from);
      return;
    }
    const back = this.alt;
    if (back.dataset.ready !== String(next.id)) {
      // 后台尚未就绪(兜底): 立即预载, 就绪后再切(此时不消费 nextClip)
      this._prepare(back, next);
      this._whenReady(back, () => this._transition());
      return;
    }
    // 瞬切: 后台已 canplaythrough 就绪(首帧可立即显示), 直接换层 + 播放
    this.nextClip = null;
    this.manualNext = null;
    front.pause();
    front.classList.remove('is-active');
    back.classList.add('is-active');
    try { back.currentTime = 0; } catch (e) { /* ignore */ }
    back.play().catch((e) => this.hooks.onStatus?.({ kind: 'error', message: `播放失败: ${e?.message || e}`, error: e }));
    this.cur = back;
    this.alt = front;
    this.currentClip = next;
    this.hooks.onStatus?.({ kind: 'switch', from, to: next });
    this._setState('playing');
    this.hooks.onTime?.(next, 0, back.duration || 0);
    this.hooks.onClipChange?.(next, back.duration || 0);
    this._refreshNext();
    // 极短片段: 切换后立即又接近末尾, 由 rAF 循环自然继续切换
  }
}
