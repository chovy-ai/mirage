#!/usr/bin/env bash
# 从 tools/AppIcon-1024.png 生成 tools/AppIcon.icns（多分辨率）。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/tools/AppIcon-1024.png"
SET="$ROOT/tools/AppIcon.iconset"

[[ -f "$SRC" ]] || python3 "$ROOT/tools/make_icon.py"
rm -rf "$SET"; mkdir -p "$SET"

gen() { sips -z "$2" "$2" "$SRC" --out "$SET/$1" >/dev/null; }
gen icon_16x16.png 16
gen icon_16x16@2x.png 32
gen icon_32x32.png 32
gen icon_32x32@2x.png 64
gen icon_128x128.png 128
gen icon_128x128@2x.png 256
gen icon_256x256.png 256
gen icon_256x256@2x.png 512
gen icon_512x512.png 512
cp "$SRC" "$SET/icon_512x512@2x.png"

iconutil -c icns "$SET" -o "$ROOT/tools/AppIcon.icns"
rm -rf "$SET"
echo "已生成： tools/AppIcon.icns"
