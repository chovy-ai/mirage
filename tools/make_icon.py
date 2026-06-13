#!/usr/bin/env python3
"""生成 mirage 应用图标：Big Sur squircle + 桌面壁纸渐变 + 菜单栏/窗口/Dock 剪影。
输出 tools/AppIcon-1024.png，再由 make_icon.sh 转成 .icns。"""
import math
from PIL import Image, ImageDraw, ImageFilter

S = 1024
SS = 4  # 超采样抗锯齿
W = S * SS
img = Image.new("RGBA", (W, W), (0, 0, 0, 0))
d = ImageDraw.Draw(img)


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


# ---- 圆角方形（Big Sur squircle 近似：大圆角）----
def squircle_mask(size, radius):
    m = Image.new("L", (size, size), 0)
    dd = ImageDraw.Draw(m)
    dd.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return m


# 壁纸竖直多段渐变（和 desktop.rs 同色）
stops = [
    (0.00, (0x0A, 0x0E, 0x26)),
    (0.42, (0x2B, 0x1E, 0x5C)),
    (0.72, (0x7A, 0x2F, 0x63)),
    (1.00, (0xE8, 0x81, 0x4F)),
]
for y in range(W):
    t = y / (W - 1)
    # 找到所在段
    for i in range(len(stops) - 1):
        t0, c0 = stops[i]
        t1, c1 = stops[i + 1]
        if t0 <= t <= t1:
            local = (t - t0) / (t1 - t0)
            col = lerp(c0, c1, local)
            break
    else:
        col = stops[-1][1]
    d.line([(0, y), (W, y)], fill=col + (255,))

# 柔光团（右下暖光 + 左上紫光）
glow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
gd = ImageDraw.Draw(glow)
gd.ellipse([int(W * 0.45), int(W * 0.55), int(W * 1.05), int(W * 1.15)],
           fill=(0xFF, 0xB0, 0x60, 90))
gd.ellipse([int(W * -0.1), int(W * 0.05), int(W * 0.4), int(W * 0.5)],
           fill=(0x6A, 0x4A, 0xC8, 70))
glow = glow.filter(ImageFilter.GaussianBlur(W // 12))
img = Image.alpha_composite(img, glow)
d = ImageDraw.Draw(img)

# ---- 顶部菜单栏 ----
mb_h = int(W * 0.085)
d.rectangle([0, 0, W, mb_h], fill=(0, 0, 0, 90))
# 苹果 logo 小圆点
d.ellipse([int(W * 0.05), int(mb_h * 0.3), int(W * 0.05) + int(mb_h * 0.4),
           int(mb_h * 0.3) + int(mb_h * 0.4)], fill=(255, 255, 255, 230))

# ---- 一个浮窗 ----
win = [int(W * 0.22), int(W * 0.27), int(W * 0.80), int(W * 0.68)]
shadow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
sd = ImageDraw.Draw(shadow)
sd.rounded_rectangle([win[0], win[1] + int(W * 0.02), win[2], win[3] + int(W * 0.03)],
                     radius=int(W * 0.03), fill=(0, 0, 0, 120))
shadow = shadow.filter(ImageFilter.GaussianBlur(W // 60))
img = Image.alpha_composite(img, shadow)
d = ImageDraw.Draw(img)
d.rounded_rectangle(win, radius=int(W * 0.03), fill=(0x22, 0x22, 0x28, 245))
# 窗口标题栏
tb_h = int(W * 0.05)
d.rounded_rectangle([win[0], win[1], win[2], win[1] + tb_h * 2],
                    radius=int(W * 0.03), fill=(0x30, 0x30, 0x37, 245))
d.rectangle([win[0], win[1] + tb_h, win[2], win[1] + tb_h * 2], fill=(0x22, 0x22, 0x28, 245))
# 红绿灯
ty = win[1] + tb_h // 2 + int(W * 0.006)
for k, c in enumerate([(0xFF, 0x5F, 0x57), (0xFE, 0xBC, 0x2E), (0x28, 0xC8, 0x40)]):
    cx = win[0] + int(W * 0.035) + k * int(W * 0.035)
    r = int(W * 0.013)
    d.ellipse([cx - r, ty - r, cx + r, ty + r], fill=c + (255,))

# ---- 底部 Dock 剪影 ----
dock_w, dock_h = int(W * 0.62), int(W * 0.11)
dock_x = (W - dock_w) // 2
dock_y = int(W * 0.80)
d.rounded_rectangle([dock_x, dock_y, dock_x + dock_w, dock_y + dock_h],
                    radius=int(dock_h * 0.32), fill=(255, 255, 255, 46))
d.rounded_rectangle([dock_x, dock_y, dock_x + dock_w, dock_y + dock_h],
                    radius=int(dock_h * 0.32), outline=(255, 255, 255, 60), width=SS * 2)
# Dock 里几个彩色小图标
icon_cols = [(0x1E, 0x9B, 0xF6), (0x2A, 0x2A, 0x30), (0x42, 0x85, 0xF4),
             (0xFC, 0x3C, 0x44), (0x32, 0xC7, 0x59), (0xFF, 0xD6, 0x0A)]
n = len(icon_cols)
pad = int(dock_h * 0.18)
slot = (dock_w - pad * 2) / n
isz = int(slot * 0.78)
for i, c in enumerate(icon_cols):
    cx = int(dock_x + pad + slot * (i + 0.5))
    cy = dock_y + dock_h // 2
    d.rounded_rectangle([cx - isz // 2, cy - isz // 2, cx + isz // 2, cy + isz // 2],
                        radius=int(isz * 0.26), fill=c + (255,))

# ---- 应用 squircle 裁剪 ----
mask = squircle_mask(W, int(W * 0.225))
out = Image.new("RGBA", (W, W), (0, 0, 0, 0))
out.paste(img, (0, 0), mask)

# 细描边增加层次
edge = Image.new("RGBA", (W, W), (0, 0, 0, 0))
ed = ImageDraw.Draw(edge)
ed.rounded_rectangle([SS, SS, W - SS, W - SS], radius=int(W * 0.225),
                     outline=(255, 255, 255, 40), width=SS * 2)
edge.putalpha(Image.composite(edge.getchannel("A"), Image.new("L", (W, W), 0), mask))
out = Image.alpha_composite(out, edge)

out = out.resize((S, S), Image.LANCZOS)
out.save("tools/AppIcon-1024.png")
print("wrote tools/AppIcon-1024.png")
