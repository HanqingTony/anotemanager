// anm-tray 主进程：窗口 / 托盘 / 全局热键 / IPC 转发（TCP → anm-core）。
//
// 架构：渲染进程（renderer/index.html）负责全部 UI 与业务；
// 主进程只做渲染进程做不到的系统集成：
//   - 透明全屏置顶窗口（覆盖层）
//   - 全局热键 Alt+Shift+Z（显示/隐藏切换）
//   - 系统托盘（激活 / 退出）
//   - TCP 转发：渲染进程 invoke → 主进程连 anm-core（协议信封与 anm_core::protocol 一致）
//
// 所有系统能力都是 Electron 官方成熟 API，无中间壳层。

const { app, BrowserWindow, Tray, Menu, globalShortcut, ipcMain, screen } = require('electron');
const net = require('net'); // Node TCP
const path = require('path');

let win = null;
let tray = null;

// anm-core 服务地址/令牌（渲染进程可经 anm-set-config 更新）
let serverAddr = process.env.ANM_SERVER_ADDR || '127.0.0.1:17370';
let serverToken = process.env.ANM_SERVER_TOKEN || null;

function createWindow() {
  // 关键：BrowserWindow 尺寸是逻辑像素（DIP）。在 125% 缩放下写死 1920x1080
  // 会得到物理 2400x1350 的超大窗口——右/下超出屏幕，鼠标落到屏幕外的
  // 窗口区域会穿透到下层程序（"拖到右/下边缘卡死"的真身）。
  // 正确做法：用主显示器逻辑尺寸（1536x864 逻辑 = 1920x1080 物理 = 全屏）。
  const { width, height } = screen.getPrimaryDisplay().bounds;
  win = new BrowserWindow({
    width,
    height,
    x: 0,
    y: 0,
    transparent: true,
    frame: false,
    alwaysOnTop: true,
    resizable: false,
    skipTaskbar: true,
    fullscreenable: false,
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  win.loadFile(path.join(__dirname, 'renderer', 'index.html'));
  // 覆盖任务栏：普通置顶窗口在任务栏之下——用真全屏模式
  win.setFullScreen(true);
  win.setAlwaysOnTop(true, 'screen-saver');

  // 窗口显隐变化通知渲染进程（刷新数据等）
  win.on('show', () => win.webContents.send('anm-event', 'shown'));
  win.on('hide', () => win.webContents.send('anm-event', 'hidden'));

  // 透明窗口防边缘问题：无
}

function showWindow() {
  if (!win) return;
  win.show();
  win.focus();
}
function hideWindow() {
  if (win) win.hide();
}
function toggleWindow() {
  if (!win) return;
  if (win.isVisible()) hideWindow();
  else showWindow();
}

// 单例
const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.quit();
} else {
  app.on('second-instance', () => showWindow());
}

app.whenReady().then(() => {
  createWindow();

  // 全局热键（与 win32 版一致：Alt+Shift+Z）
  globalShortcut.register('Alt+Shift+Z', toggleWindow);

  // 系统托盘
  tray = new Tray(path.join(__dirname, 'renderer', 'appIcon.png'));
  tray.setToolTip('anm-tray');
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: '激活', click: () => showWindow() },
      { type: 'separator' },
      { label: '退出', click: () => app.exit(0) },
    ])
  );
  tray.on('click', () => showWindow());

  app.on('will-quit', () => {
    globalShortcut.unregisterAll();
  });
});

// IPC：更新服务地址/令牌（渲染进程设置对话框用）
ipcMain.handle('anm-set-config', (_e, cfg) => {
  if (cfg && typeof cfg.addr === 'string' && cfg.addr.trim()) {
    serverAddr = cfg.addr.trim();
  }
  if (cfg && typeof cfg.token === 'string') {
    serverToken = cfg.token.trim() || null;
  }
  return { ok: true, addr: serverAddr };
});

// IPC：TCP 转发到 anm-core（协议信封与 anm_core::protocol 序列化一致）
ipcMain.handle('anm-ipc', (_e, req) => {
  const cmd = (req && req.cmd) || '';
  const params = (req && req.params) || {};
  const addr = (req && req.addr) || serverAddr;
  const token = (req && req.token !== undefined) ? req.token : serverToken;
  return ipcCall(addr, token, cmd, params);
});

function ipcCall(addr, token, cmd, params) {
  return new Promise((resolve) => {
    // 信封
    const request = { cmd };
    const hasParams = params && !(typeof params === 'object' && Object.keys(params).length === 0);
    if (hasParams) request.params = params;
    const envelope = JSON.stringify({ token: token || null, request });

    const [host, portStr] = addr.split(':');
    const port = parseInt(portStr, 10);
    const sock = net.connect({ host, port }, () => {
      sock.write(envelope + '\n');
    });
    const timeout = setTimeout(() => {
      sock.destroy();
      resolve({ status: 'error', data: 'IPC 超时: ' + addr });
    }, 8000);
    let buf = '';
    sock.on('data', (chunk) => {
      buf += chunk.toString('utf8');
      const nl = buf.indexOf('\n');
      if (nl >= 0) {
        clearTimeout(timeout);
        sock.destroy();
        try {
          const resp = JSON.parse(buf.slice(0, nl));
          if (resp.ok) resolve({ status: 'ok', data: resp.data });
          else resolve({ status: 'error', data: resp.error || '服务错误' });
        } catch (e) {
          resolve({ status: 'error', data: '响应解析失败: ' + e.message });
        }
      }
    });
    sock.on('error', (e) => {
      clearTimeout(timeout);
      resolve({ status: 'error', data: '无法连接 anm-core ' + addr + ': ' + e.message });
    });
  });
}
