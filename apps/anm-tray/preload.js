// preload：把主进程能力安全暴露给渲染进程。
// 渲染进程通过 window.anm 使用：
//   anm.ipc(cmd, params) → Promise（TCP 转发到 anm-core）
//   anm.setConfig({addr, token})
//   anm.onEvent(cb)      → 窗口显隐等系统事件（cb(event, detail)）
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('anm', {
  ipc: (cmd, params) => ipcRenderer.invoke('anm-ipc', { cmd, params }),
  setConfig: (cfg) => ipcRenderer.invoke('anm-set-config', cfg),
  onEvent: (cb) => {
    ipcRenderer.on('anm-event', (_e, event) => cb(event));
  },
});
