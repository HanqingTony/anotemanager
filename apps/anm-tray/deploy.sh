#!/bin/bash
# anm-tray (Electron) 部署到 101
set -e
cd "$(dirname "$0")"

# 1) 解压 electron 预编译包（若未解压）
if [ ! -d /tmp/electron-dist/electron.exe ]; then
  rm -rf /tmp/electron-dist && mkdir -p /tmp/electron-dist
  cd /tmp/electron-dist && unzip -o -q /tmp/electron.zip
fi

# 2) 打包 app 目录
cd /home/tony/zrepo/anotemanager/apps/anm-tray
tar czf /tmp/anm-electron-app.tgz main.js preload.js package.json renderer

# 3) 传 101
scp -q /tmp/anm-electron-app.tgz tony@192.168.0.101:/tmp/
# electron 二进制只传一次
ssh tony@192.168.0.101 "test -d /mnt/c/Users/hanqi/anm-tray-electron && echo EXE_EXISTS || echo EXE_MISSING" | grep -q EXE_EXISTS || {
  cd /tmp/electron-dist && tar czf /tmp/electron-bin.tgz . && scp -q /tmp/electron-bin.tgz tony@192.168.0.101:/tmp/
}

# 4) 101 布局
timeout 40 ssh tony@192.168.0.101 '
  set -e
  cd /mnt/c/Users/hanqi
  mkdir -p anm-tray-electron
  cd anm-tray-electron
  if [ ! -f electron.exe ]; then
    tar xzf /tmp/electron-bin.tgz
  fi
  rm -rf app && mkdir app
  cd app && tar xzf /tmp/anm-electron-app.tgz
  # 杀掉旧实例后启动
  /mnt/c/Windows/System32/taskkill.exe /F /IM electron.exe > /dev/null 2>&1 || true
  sleep 1
  /mnt/c/Windows/explorer.exe "C:\\Users\\hanqi\\anm-tray-electron\\electron.exe" "C:\\Users\\hanqi\\anm-tray-electron\\app"
  sleep 6
  /mnt/c/Windows/System32/tasklist.exe /FI "IMAGENAME eq electron.exe" 2>/dev/null | tail -2
'
echo "部署完成"
