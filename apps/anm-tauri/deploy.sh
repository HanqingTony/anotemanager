#!/usr/bin/env bash
# 部署 anm-tauri 到 101（Windows）：杀旧进程 → 传 exe + WebView2Loader.dll + renderer/ → 启动
#
# 前端为外部目录模式：renderer/ 与 exe 同目录部署，改前端零编译
# （用 deploy-front.sh 单独推送）。
#
# 注意：交叉编译的 exe 在导入表里引用 WebView2Loader.dll（webview2-com-sys
# 对非 MSVC 目标用动态 loader），必须与 exe 同目录部署，否则启动即弹
# "WebView2Loader.dll was not found" 并退出。
set -u
HOST=tony@192.168.0.101
EXE=apps/anm-tauri/src-tauri/target/x86_64-pc-windows-gnu/release/anm-tauri.exe
LOADER=apps/anm-tauri/WebView2Loader.dll
RENDERER=apps/anm-tauri/renderer
DEST=/mnt/c/Users/hanqi/anm-tauri

if [ ! -f "$EXE" ]; then
  echo "未找到 $EXE，先构建"; exit 1
fi

scp -q "$EXE" "$HOST:/tmp/anm-tauri.exe" || { echo scp exe 失败; exit 1; }
scp -q "$LOADER" "$HOST:/tmp/WebView2Loader.dll" || { echo scp loader 失败; exit 1; }
timeout 30 ssh "$HOST" "rm -rf /tmp/anm-renderer" 2>/dev/null
scp -qr "$RENDERER" "$HOST:/tmp/anm-renderer" || { echo scp renderer 失败; exit 1; }

timeout 60 ssh "$HOST" "
  /mnt/c/Windows/System32/taskkill.exe /F /IM anm-tauri.exe > /dev/null 2>&1
  sleep 1
  mkdir -p $DEST
  cp /tmp/anm-tauri.exe $DEST/anm-tauri.exe
  cp /tmp/WebView2Loader.dll $DEST/WebView2Loader.dll
  rm -rf $DEST/renderer
  cp -r /tmp/anm-renderer $DEST/renderer
  cd $DEST
  setsid nohup ./anm-tauri.exe > /tmp/anm-tauri-run.log 2>&1 < /dev/null & disown
  sleep 10
  /mnt/c/Windows/System32/tasklist.exe /FI \"IMAGENAME eq anm-tauri.exe\" 2>/dev/null | tail -2
  echo '--- panic.log ---'; cat $DEST/panic.log 2>/dev/null || echo '(无 panic)'
  echo '--- renderer ---'; ls $DEST/renderer/
" 2>&1
echo "部署完成"
