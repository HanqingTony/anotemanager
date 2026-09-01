// fps.js — 帧率检测(仅供"逐帧预览"内部使用, 用户不需要关心 fps)
// 通过真实播放 + requestVideoFrameCallback 测量相邻呈现帧的 mediaTime 间隔,
// 取中位数倒数作为帧率。检测失败时由调用方回退到 30。

export function probeVideo(blob, { maxFrames = 30, timeoutMs = 4000 } = {}) {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(blob);
    const v = document.createElement('video');
    v.muted = true;
    v.playsInline = true;
    v.preload = 'auto';
    v.src = url;
    v.style.cssText = 'position:fixed;left:-9999px;top:0;width:4px;height:4px;opacity:0;pointer-events:none;';
    document.body.appendChild(v);

    const deltas = [];
    let last = null;
    let done = false;

    const computeFps = () => {
      const sorted = [...deltas].sort((a, b) => a - b);
      if (!sorted.length) return null;
      const median = sorted[Math.floor(sorted.length / 2)];
      if (!(median > 1e-4)) return null;
      const raw = 1 / median;
      return raw >= 1 && raw <= 240 ? Math.round(raw * 1000) / 1000 : null;
    };

    const cleanup = () => {
      if (done) return;
      done = true;
      v.pause();
      v.removeAttribute('src');
      v.load();
      v.remove();
      URL.revokeObjectURL(url);
    };
    const finish = () => {
      clearTimeout(timer);
      cleanup();
      resolve(computeFps());
    };

    const timer = setTimeout(finish, timeoutMs);

    v.addEventListener('ended', finish);

    if (!('requestVideoFrameCallback' in v)) {
      v.addEventListener('loadeddata', finish, { once: true });
      v.play().catch(() => finish());
      return;
    }

    const tick = (now, meta) => {
      if (done) return;
      if (last !== null) {
        const d = meta.mediaTime - last;
        if (d > 1e-4 && d < 0.5) deltas.push(d);
      }
      last = meta.mediaTime;
      if (deltas.length >= maxFrames) { finish(); return; }
      v.requestVideoFrameCallback(tick);
    };
    v.requestVideoFrameCallback(tick);
    v.play().catch(() => {});
  });
}
