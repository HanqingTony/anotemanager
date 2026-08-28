#!/usr/bin/env bash
# 只推送前端（renderer/）到 101 并重启应用——零编译前端迭代。
# 用法：改完 apps/anm-tauri/renderer/ 后直接跑本脚本。
set -u
HOST=tony@192.168.0.101
RENDERER=apps/anm-tauri/renderer
DEST=/mnt/c/Users/tony/anm-tauri

timeout 30 ssh "$HOST" "rm -rf /tmp/anm-renderer" 2>/dev/null
scp -qr "$RENDERER" "$HOST:/tmp/anm-renderer" || { echo scp renderer 失败; exit 1; }

timeout 60 ssh "$HOST" "
  /mnt/c/Windows/System32/taskkill.exe /F /IM anm-tauri.exe > /dev/null 2>&1
  sleep 1
  rm -rf $DEST/renderer
  cp -r /tmp/anm-renderer $DEST/renderer
  cd $DEST
  setsid nohup ./anm-tauri.exe > /tmp/anm-tauri-run.log 2>&1 < /dev/null & disown
  sleep 8
  /mnt/c/Windows/System32/tasklist.exe /FI \"IMAGENAME eq anm-tauri.exe\" 2>/dev/null | tail -2
  echo '--- panic.log ---'; cat $DEST/panic.log 2>/dev/null || echo '(无 panic)'
" 2>&1
echo "前端已推送并重启"
