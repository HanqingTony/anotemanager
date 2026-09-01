// e2e.mjs — 无头浏览器端到端自检
// 前置: 静态服务器已在指定端口(默认 8137), 本机有 playwright-core 与缓存的 chromium。
// 用法: node demo/e2e.mjs [port]
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
let chromium = null;
const PW = '/home/tony/zrepo/deepseek-harness/node_modules/.pnpm/playwright-core@1.61.1/node_modules/playwright-core/index.js';
try {
  ({ chromium } = require(PW));
} catch (e) {
  console.log('SKIP: playwright-core 不可用 (' + e.message + ')');
  process.exit(0);
}

const port = Number(process.argv[2] || process.env.E2E_PORT || 8137);
const base = `http://127.0.0.1:${port}/`;
const EXE = process.env.HOME + '/.cache/ms-playwright/chromium-1234/chrome-linux64/chrome';

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function assert(cond, msg) {
  if (!cond) throw new Error('断言失败: ' + msg);
  console.log('  ✓ ' + msg);
}

// 按名称点击片段卡片(未播放=播放, 播放中=指定下一段)
async function clickClipByName(page, name) {
  const id = await page.evaluate((n) => {
    const c = window.alooplayer.getClips().find((x) => x.name === n);
    return c ? c.id : null;
  }, name);
  if (!id) throw new Error('找不到片段: ' + name);
  await page.click(`.clip-card[data-id="${id}"]`);
}

// 停止播放器(直接调用引擎 API, 不依赖按钮状态, 避免 UI 竞态)
async function stopPlayer(page) {
  await page.evaluate(() => window.alooplayer.player.stop());
  await sleep(50);
}

const browser = await chromium.launch({
  executablePath: EXE,
  headless: true,
  args: ['--autoplay-policy=no-user-gesture-required', '--mute-audio'],
});

try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push('pageerror: ' + e.message));
  page.on('console', (m) => { if (m.type() === 'error') errors.push('console: ' + m.text()); });

  await page.goto(base, { waitUntil: 'networkidle', timeout: 20000 });
  console.log('· 页面已加载:', await page.title());

  // ---------- 0. 加载演示片段 + 卡片结构 ----------
  await page.click('#btn-demo');
  await page.waitForFunction(() => document.querySelectorAll('.clip-card').length === 6, null, { timeout: 25000 });
  assert(true, '加载演示片段: 6 个片段卡片出现');
  const struct = await page.evaluate(() => {
    const card = document.querySelector('.clip-card');
    const r = card.getBoundingClientRect();
    const nums = [...card.querySelectorAll('.clip-row2 .num')];
    return {
      thumb: !!card.querySelector('.clip-thumb'),
      name: !!card.querySelector('.clip-name'),
      numType: nums[0]?.type,
      numMode: nums[0]?.inputMode,
      del: !!card.querySelector('[data-act="del"]'),
      h: Math.round(r.height),
    };
  });
  assert(struct.thumb && struct.numType === 'text' && struct.numMode === 'numeric' && struct.del && struct.h <= 70,
    `行式小卡片: 缩略图+两行信息+删除按钮, 高度 ${struct.h}px; 编号为纯文本数字输入(无上下箭头)`);

  // 读取当前 (片段名, 当前时间秒[进度条浮点值], 总时长秒)
  const snap = () => page.evaluate(() => {
    const badge = document.querySelector('#cur-badge').textContent.trim();
    const seek = document.querySelector('#seek');
    const name = badge.split(' ')[0];
    return {
      name,
      t: seek && parseFloat(seek.max) > 0 ? parseFloat(seek.value) : null,
      dur: seek ? parseFloat(seek.max) : null,
    };
  });

  // ---------- 1. 顺序接续: 整条链 A→B→C→D→E 逐个衔接 ----------
  await page.click('[data-strategy="sequential"]');
  await clickClipByName(page, 'A · 帧1-3'); // 未播放时点击卡片 = 播放
  await page.waitForFunction(() => /\/ \d+:\d+/.test(document.querySelector('#frame-info').textContent), null, { timeout: 8000 });
  const samples = [];
  const t0 = Date.now();
  while (Date.now() - t0 < 4000) {
    const s = await snap();
    if (s.name) samples.push(s);
    await sleep(20); // 短片段在屏窗口短(提前切换), 加密采样避免错过
  }
  const seen = new Set(samples.map((s) => s.name));
  assert(seen.has('A') && seen.has('B') && seen.has('C') && seen.has('D') && seen.has('E'),
    `顺序接续: 片段链自动推进 A→B→C→D→E (观察到: ${[...seen].join(',')})`);
  const badTime = samples.filter((s) => s.t !== null && s.t < 0);
  assert(badTime.length === 0, `顺序接续: 时间读数正常 (${samples.length} 次采样)`);
  // 播放中: 当前卡片缩略图上应显示 ▶ 播放图标
  const playIcon = await page.evaluate(() => {
    const badge = document.querySelector('.clip-card.playing .thumb-badge');
    return badge ? getComputedStyle(badge).opacity : null;
  });
  assert(playIcon === '1', '正在播放: 当前片段缩略图显示 ▶ 播放图标');

  // ---------- 2. 循环当前: A 反复播放, 时间不断回卷 ----------
  await stopPlayer(page);
  await page.click('[data-strategy="loop"]');
  await clickClipByName(page, 'A · 帧1-3');
  await page.waitForFunction(() => /\/ \d+:\d+/.test(document.querySelector('#frame-info').textContent), null, { timeout: 8000 });
  await sleep(800);
  const seq = [];
  for (let i = 0; i < 30; i++) {
    const s = await snap();
    if (s.t !== null) seq.push(s.t);
    await sleep(60);
  }
  const wraps = seq.filter((v, i) => i > 0 && v < seq[i - 1]).length;
  assert(wraps >= 2 && seq.length > 10, `循环当前: 时间持续循环回卷 (${wraps} 次回卷)`);

  // ---------- 3. 随机接续: 绿框自动指示下一个, 且从 A 切到可接续片段(B/F) ----------
  await stopPlayer(page);
  await page.click('[data-strategy="random"]');
  await clickClipByName(page, 'A · 帧1-3');
  await page.waitForFunction(() => /\/ \d+:\d+/.test(document.querySelector('#frame-info').textContent), null, { timeout: 8000 });
  await page.waitForFunction(() => document.querySelectorAll('.clip-card.selected').length >= 1, null, { timeout: 8000 });
  const selName = await page.evaluate(() => document.querySelector('.clip-card.selected .clip-name')?.value);
  const selIcon = await page.evaluate(() => getComputedStyle(document.querySelector('.clip-card.selected .thumb-badge')).opacity);
  assert(selName && /B ·|F ·/.test(selName) && selIcon === '1',
    `随机接续: 即将播放的片段缩略图显示 ⏳ 等待图标 (${selName})`);
  await page.waitForFunction(() => /B ·|F ·/.test(document.querySelector('#cur-badge').textContent), null, { timeout: 8000 });
  assert(true, '随机接续: 从 A 切换到可无缝接续片段 (' + (await page.textContent('#cur-badge')).trim() + ')');
  await sleep(500);

  // ---------- 4. 可接续置顶 + 手动点选: 播放 C 时 D 绿框置顶, 点击 D 变 ⏳, 播完切换 ----------
  await stopPlayer(page);
  await page.click('[data-strategy="manual"]');
  await clickClipByName(page, 'C · 帧4-7');
  await page.waitForFunction(() => /C ·/.test(document.querySelector('#cur-badge').textContent), null, { timeout: 8000 });
  // 立即暂停(直接调 API): 让"点选下一段"的反馈稳定可见, 避免与播完切换竞态
  const paused = await page.evaluate(() => {
    const p = window.alooplayer.player;
    if (p.state === 'playing') p.pause();
    return p.state === 'paused';
  });
  // C 的可接续片段 D 应为绿框且置顶(列表第一个)
  await page.waitForFunction(() => {
    const c = document.querySelector('.clip-card.connectable .clip-name');
    return c && c.value.includes('D · 帧7-9');
  }, null, { timeout: 5000 });
  const order = await page.evaluate(() => {
    return [...document.querySelectorAll('.clip-card')]
      .sort((a, b) => a.getBoundingClientRect().top - b.getBoundingClientRect().top)
      .map((c) => c.querySelector('.clip-name').value);
  });
  assert(order[0].includes('D · 帧7-9'), `可接续片段置顶: 视觉第一项为 D (实际: ${order.slice(0, 3).join(' | ')})`);
  // 点击 D 卡片 = 指定下一段(缩略图出现等待图标)
  await clickClipByName(page, 'D · 帧7-9');
  if (paused) {
    await page.waitForFunction(() => {
      const c = document.querySelector('.clip-card.selected .clip-name');
      return c && c.value.includes('D · 帧7-9');
    }, null, { timeout: 5000 });
    const dIcon = await page.evaluate(() => getComputedStyle(document.querySelector('.clip-card.selected .thumb-badge')).opacity);
    assert(dIcon === '1', '手动指定: 点击 D 卡片后缩略图出现 ⏳ 等待图标');
    await page.evaluate(() => window.alooplayer.player.resume()); // 恢复播放
  } else {
    console.log('  (C 已播完, 走 need-select 点选路径)');
  }
  await page.waitForFunction(() => /D ·/.test(document.querySelector('#cur-badge').textContent), null, { timeout: 8000 });
  assert(true, '手动指定: C 播完后切换到所选片段 D');

  // ---------- 5. 播完等待: 手动模式不选择, 弹出选择浮层 ----------
  await stopPlayer(page);
  await clickClipByName(page, 'A · 帧1-3'); // 手动模式下不做任何选择
  await page.waitForSelector('#overlay-need:not(.hidden)', { timeout: 8000 });
  assert(true, '手动未选择: 播完后弹出选择浮层等待');
  await page.click('#need-loop');
  await sleep(400);
  assert(await page.isHidden('#overlay-need'), '点击「循环当前」后继续播放');

  // ---------- 6. 用户场景(回归测试): 全新页面添加两个普通长视频, 切换节奏必须正常 ----------
  // 曾出现 bug: 添加两个片段后以超高速度连切 —— 每个片段应完整播完再切换
  const page2 = await browser.newPage();
  const errors2 = [];
  page2.on('pageerror', (e) => errors2.push('pageerror: ' + e.message));
  page2.on('console', (m) => { if (m.type() === 'error') errors2.push('console: ' + m.text()); });
  await page2.goto(base, { waitUntil: 'networkidle', timeout: 20000 });
  // 记录切换触发时机: 应提前于 ended(剩余时间≤提前量), 且换层时后台已完整就绪
  await page2.evaluate(() => {
    const p = window.alooplayer.player;
    window.__trans = [];
    const ot = p._transition.bind(p);
    p._transition = function () {
      window.__trans.push({
        t: +this.cur.currentTime.toFixed(3),
        dur: +this.cur.duration.toFixed(3),
        ended: this.cur.ended,
        backReady: this.alt.dataset.ready === String(this.nextClip?.id || ''),
        backRs: this.alt.readyState,
      });
      ot();
    };
  });
  async function fetchBuf(p) {
    const r = await page2.evaluate(async (path) => {
      const res = await fetch(path);
      return Array.from(new Uint8Array(await res.arrayBuffer()));
    }, p);
    return Buffer.from(r);
  }
  await page2.setInputFiles('#file-input', [
    { name: '长视频A.mp4', mimeType: 'video/mp4', buffer: await fetchBuf('demo/clips/long-a.mp4') }, // 2s
    { name: '长视频B.mp4', mimeType: 'video/mp4', buffer: await fetchBuf('demo/clips/long-b.mp4') }, // 3s
  ]);
  await page2.waitForFunction(() => document.querySelectorAll('.clip-card').length === 2, null, { timeout: 8000 });
  // 设置连接标签: A 结束 101 == B 起始 101; B 结束 101 == 起始 101(自接续, 播完接自己)
  await page2.fill('.clip-card:nth-child(1) input[data-field="end"]', '101');
  await page2.press('.clip-card:nth-child(1) input[data-field="end"]', 'Enter');
  await page2.fill('.clip-card:nth-child(2) input[data-field="start"]', '101');
  await page2.press('.clip-card:nth-child(2) input[data-field="start"]', 'Enter');
  await page2.fill('.clip-card:nth-child(2) input[data-field="end"]', '101');
  await page2.press('.clip-card:nth-child(2) input[data-field="end"]', 'Enter');
  await sleep(300);
  const labels = await page2.evaluate(() => window.alooplayer.getClips().map((c) => `${c.name}:${c.startFrame}-${c.endFrame}`));
  assert(/长视频A:1-101/.test(labels) && /长视频B:101-101/.test(labels),
    `用户场景: 连接标签已设置 (${labels.join(', ')})`);
  await page2.click('[data-strategy="sequential"]');
  await clickClipByName(page2, '长视频A'); // 未播放时点击卡片 = 播放
  await page2.waitForFunction(() => /长视频A/.test(document.querySelector('#cur-badge').textContent), null, { timeout: 8000 });
  const snap2 = () => page2.evaluate(() => document.querySelector('#cur-badge').textContent.trim().split(' ')[0]);
  const changes = [];
  let last = '';
  const t1 = Date.now();
  while (Date.now() - t1 < 9000) {
    const name = await snap2();
    if (name && name !== last) {
      if (last) changes.push({ from: last, to: name, at: Date.now() - t1 });
      last = name;
    }
    await sleep(80);
  }
  const intervals = changes.map((c, i) => i === 0 ? c.at : c.at - changes[i - 1].at);
  assert(changes.length >= 1 && changes[0].from === '长视频A' && changes[0].to === '长视频B',
    `用户场景: 长视频A 播完后切换到 长视频B (变化序列: ${changes.map((c) => `${c.from}→${c.to}@${c.at}ms`).join(', ') || '无'})`);
  assert(intervals[0] >= 1000, `用户场景: A 完整播完(约2s)后才切换 (实际 ${intervals[0]}ms)`);
  assert(changes.length === 1, `用户场景: B(101-101) 自接续循环, 无多余切换 (变化次数 ${changes.length})`);
  // 自接续: B 播放时, B 卡片同时是"可接续"(绿框); 正在播放中保持 ▶ 图标
  const selfState = await page2.evaluate(() => ({
    connect: document.querySelector('.clip-card.connectable .clip-name')?.value,
    playIcon: getComputedStyle(document.querySelector('.clip-card.playing .thumb-badge')).opacity,
  }));
  assert(/长视频B/.test(selfState.connect) && selfState.playIcon === '1',
    `自接续: B(101-101) 播完后接续自己, 绿框指示在自己的卡片上, ▶ 播放图标保持`);
  // 无黑帧验证: 切换提前于 ended 触发, 且换层时后台已 canplaythrough 完整就绪
  const trans = await page2.evaluate(() => window.__trans);
  const early = trans.filter((x) => x.t < x.dur - 0.001 && x.dur > 1);
  assert(early.length >= 2, `切换时机: 提前于 ended 触发, 无黑帧窗口 (${trans.map((x) => `${x.t}/${x.dur}${x.ended ? '(ended)' : ''}`).join(', ')})`);
  assert(trans.every((x) => x.backReady && x.backRs >= 4),
    `换层安全: 后台已完整就绪(canplaythrough)后才切换 (${trans.map((x) => `rs=${x.backRs}${x.backReady ? '✓' : '✗'}`).join(', ')})`);
  assert(errors2.length === 0, '用户场景: 全程无页面错误' + (errors2.length ? ': ' + errors2.join('; ') : ''));
  await page2.close();

  // ---------- 7. 预览(点击缩略图) ----------
  await page.click('.clip-card[data-id] .clip-thumb');
  await page.waitForSelector('#preview-modal:not(.hidden)', { timeout: 5000 });
  await sleep(800); // 等内部帧率检测完成
  const f0 = (await page.textContent('#pv-frame')).trim();
  await page.click('#pv-next');
  await sleep(300);
  const f1 = (await page.textContent('#pv-frame')).trim();
  assert(f0 !== f1, `预览: 下一帧推进 (${f0} → ${f1})`);
  await page.click('#pv-capture');
  assert((await page.$$('.capture-item')).length === 1, '预览: 截帧成功');
  // 截帧下载链接: 存在且能触发真实浏览器下载
  const dlInfo = await page.evaluate(() => {
    const a = document.querySelector('.capture-item .capture-dl');
    return { href: a?.href || '', name: a?.download || '', text: a?.textContent || '' };
  });
  assert(dlInfo.href.startsWith('data:image/jpeg') && dlInfo.name.endsWith('.jpg') && /下载/.test(dlInfo.text),
    `截帧下载链接: dataURL + .jpg 文件名 (${dlInfo.name})`);
  const [download] = await Promise.all([
    page.waitForEvent('download', { timeout: 5000 }),
    page.click('.capture-item .capture-dl'),
  ]);
  assert(download.suggestedFilename().endsWith('.jpg'), `截帧下载: 触发浏览器下载 (${download.suggestedFilename()})`);
  await page.click('#pv-close');

  await page.screenshot({ path: 'demo/screenshot.png' });

  assert(errors.length === 0, '全程无页面错误/控制台错误' + (errors.length ? ':\n  ' + errors.join('\n  ') : ''));
  console.log('\nE2E 全部通过 ✅  截图: demo/screenshot.png');
} finally {
  await browser.close();
}
