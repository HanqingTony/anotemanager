#!/usr/bin/env bash
# 生成无缝衔接演示片段:
# 先渲染帧 1..11 的逐帧 PNG(画面 = 深色背景 + 大号帧号), 再从 PNG 序列中截取片段。
# 共享帧(如 A 的帧3 与 B 的帧3)使用同一张 PNG 编码, 像素完全一致,
# 因此 A→B 衔接时画面零变化, 可直观验证"无缝衔接"。
#
# 片段关系(帧号即画面上的数字):
#   A(1-3) → B(3-4) → C(4-7) → D(7-9) → E(9-11)  主链
#   A(1-3) → F(3-6)                                分支(死胡同, 演示无可接续时循环)
set -euo pipefail
cd "$(dirname "$0")"

FPS=30
W=640
H=360
WORK="work"
OUT="clips"

command -v ffmpeg >/dev/null 2>&1 || { echo "错误: 需要安装 ffmpeg"; exit 1; }
# 注意: pipefail 下不能用 grep -q(提前关闭管道会让 ffmpeg 收到 SIGPIPE 被判失败)
if ! ffmpeg -hide_banner -filters 2>/dev/null | grep drawtext >/dev/null; then
  echo "错误: 本机 ffmpeg 缺少 drawtext 滤镜, 无法生成演示片段"
  exit 1
fi

# 找可用字体
FONT=""
if command -v fc-match >/dev/null 2>&1; then
  FONT=$(fc-match -f '%{file}' 'DejaVu Sans:bold' 2>/dev/null || true)
fi
if [ -z "$FONT" ] || [ ! -f "$FONT" ]; then
  FONT=$(find /usr/share/fonts -name '*.ttf' 2>/dev/null | head -n1 || true)
fi
if [ -z "$FONT" ] || [ ! -f "$FONT" ]; then
  echo "错误: 找不到可用字体"
  exit 1
fi
echo "使用字体: $FONT"

rm -rf "$WORK" "$OUT"
mkdir -p "$WORK" "$OUT"

# 渲染帧 1..11 为 PNG
for i in $(seq 1 11); do
  n=$(printf '%02d' "$i")
  FILTER="drawtext=text='$i':fontfile=$FONT:fontsize=240:fontcolor=white:x=(w-text_w)/2:y=(h-text_h)/2-40,drawtext=text='FRAME $i':fontfile=$FONT:fontsize=30:fontcolor=0x8fa3bf:x=(w-text_w)/2:y=h-86,drawtext=text='30 fps':fontfile=$FONT:fontsize=20:fontcolor=0x4a5a75:x=w-text_w-28:y=h-52"
  ffmpeg -y -loglevel error -f lavfi -i "color=c=0x10151f:s=${W}x${H}:r=1:d=1" \
    -vf "$FILTER" -frames:v 1 "$WORK/f$n.png"
done

# 编码片段: mk <输出名> <起始帧号> <帧数>
# 用 -crf 0(无损): 重建像素与源完全一致, 保证共享帧在不同片段中逐字节相同
# (有损模式即使同帧同参数, x264 的码率控制也会因所在流不同产生细微差异)
mk() {
  local name="$1" start="$2" count="$3"
  ffmpeg -y -loglevel error -framerate "$FPS" -start_number "$start" -i "$WORK/f%02d.png" \
    -frames:v "$count" -c:v libx264 -preset slow -crf 0 -pix_fmt yuv420p -movflags +faststart \
    "$OUT/$name.mp4"
}
mk clip-a 1 3
mk clip-b 3 2
mk clip-c 4 4
mk clip-d 7 3
mk clip-e 9 3
mk clip-f 3 4

# 普通长视频(模拟用户自己的视频文件, 供 e2e 验证切换节奏, 不在演示清单中)
ffmpeg -y -loglevel error -f lavfi -i "testsrc2=size=640x360:rate=30" -t 2 -c:v libx264 -preset fast -crf 23 -pix_fmt yuv420p -movflags +faststart "$OUT/long-a.mp4"
ffmpeg -y -loglevel error -f lavfi -i "testsrc2=size=640x360:rate=30" -t 3 -c:v libx264 -preset fast -crf 23 -pix_fmt yuv420p -movflags +faststart "$OUT/long-b.mp4"

# 生成清单(应用内"加载演示片段"按钮读取)
cat > manifest.json <<'EOF'
{
  "name": "演示片段链 (帧 1→11)",

  "clips": [
    { "file": "clip-a.mp4", "name": "A · 帧1-3",   "startFrame": 1, "endFrame": 3 },
    { "file": "clip-b.mp4", "name": "B · 帧3-4",   "startFrame": 3, "endFrame": 4 },
    { "file": "clip-c.mp4", "name": "C · 帧4-7",   "startFrame": 4, "endFrame": 7 },
    { "file": "clip-d.mp4", "name": "D · 帧7-9",   "startFrame": 7, "endFrame": 9 },
    { "file": "clip-e.mp4", "name": "E · 帧9-11",  "startFrame": 9, "endFrame": 11 },
    { "file": "clip-f.mp4", "name": "F · 帧3-6",   "startFrame": 3, "endFrame": 6 }
  ]
}
EOF

rm -rf "$WORK"

echo "完成:"
for f in "$OUT"/*.mp4; do
  ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of csv=p=0 "$f" | xargs -I{} echo "  $(basename "$f") · {} 帧"
done
echo "清单: manifest.json"
