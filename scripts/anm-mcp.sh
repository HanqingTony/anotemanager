#!/usr/bin/env bash
# anm MCP LAN 入口管理：nginx 转发 192.168.0.102:17372/mcp → 127.0.0.1:17371/mcp
# MCP 服务本身只绑回环（readme §17）；本脚本把入口配置写入 nginx 并确保 core 运行。
# 用法：
#   bash scripts/anm-mcp.sh          确保配置 + core 运行（幂等，可重复执行）
#   bash scripts/anm-mcp.sh status   只查看状态
set -u
CONF=/etc/nginx/conf.d/anm-mcp.conf
TPL="$(dirname "$0")/anm-mcp.conf"
LISTEN_IP=192.168.0.102
LISTEN_PORT=17372
CORE_DIR=/home/tony/zrepo/anotemanager
CORE_BIN=./target/debug/anm-core

echo "== anm MCP 入口管理 =="

# 1) nginx 配置（模板随仓库维护，可随时恢复）
if [ -f "$TPL" ]; then
  sudo cp "$TPL" "$CONF"
  echo "已写入 nginx 配置: $CONF"
else
  echo "缺少模板 $TPL，跳过 nginx 配置（仅检查 core）"
fi
sudo nginx -t > /dev/null 2>&1 || { echo "nginx 配置语法错误，abort"; exit 1; }
sudo nginx -s reload > /dev/null 2>&1
echo "nginx 已 reload"

# 2) anm-core（未运行则启动）
if pgrep -f "target/debug/anm-core" > /dev/null; then
  echo "anm-core 运行中"
else
  echo "anm-core 未运行，启动…"
  (cd "$CORE_DIR" && setsid nohup $CORE_BIN > /tmp/anm-core.log 2>&1 < /dev/null & disown)
  sleep 2
  pgrep -f "target/debug/anm-core" > /dev/null && echo "anm-core 已启动" || { echo "anm-core 启动失败，看 /tmp/anm-core.log"; exit 1; }
fi

# 3) 自检 LAN 入口
if [ "${1:-}" != "status" ]; then
  code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "http://$LISTEN_IP:$LISTEN_PORT/mcp" \
    -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
    -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"check","version":"1"}}}' 2>/dev/null)
  if [ "$code" = "200" ]; then
    echo "LAN 入口自检: HTTP $code OK"
  else
    echo "LAN 入口自检: HTTP $code 异常"
  fi
fi
echo "== 完成：MCP LAN 入口 http://$LISTEN_IP:$LISTEN_PORT/mcp =="
echo "   本机回环      http://127.0.0.1:17371/mcp"
echo "   远程访问      ssh -L 17371:127.0.0.1:17371 tony@$LISTEN_IP 后连回环地址"
