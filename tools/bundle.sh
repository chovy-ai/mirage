#!/usr/bin/env bash
# 把 release 二进制打包成可双击运行的 macOS .app，输出到 dist/。
# 用法：tools/bundle.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/Mirage.app"
BIN="$ROOT/target/release/mirage"

if [[ ! -f "$BIN" ]]; then
  echo "未找到 release 二进制，请先： cargo build --release" >&2
  exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/mirage"

# 应用图标（缺失则现生成）
[[ -f "$ROOT/tools/AppIcon.icns" ]] || "$ROOT/tools/make_icon.sh"
cp "$ROOT/tools/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Mirage</string>
    <key>CFBundleDisplayName</key><string>Mirage</string>
    <key>CFBundleIdentifier</key><string>com.team-shell.mirage</string>
    <key>CFBundleExecutable</key><string>mirage</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSAppNapIsDisabled</key><true/>
    <key>NSAppSleepDisabled</key><true/>
</dict>
</plist>
PLIST

# ad-hoc 签名，便于本机直接运行
codesign --force --deep -s - "$APP" >/dev/null 2>&1 || true

echo "已生成： $APP"
du -sh "$APP" | awk '{print "体积： "$1}'
