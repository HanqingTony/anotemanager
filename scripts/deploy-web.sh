#!/usr/bin/env bash
# 同步 anm-web 到 nginx 部署目录（浏览器访问 http://192.168.0.102:18101）
set -u
sudo cp /home/tony/zrepo/anotemanager/apps/anm-web/index.html /var/www/anm-web/index.html && sudo chmod 644 /var/www/anm-web/index.html
echo "anm-web 已同步 → http://192.168.0.102:18101"
