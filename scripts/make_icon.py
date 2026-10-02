#!/usr/bin/env python3
"""wp8f logo 生成器：涡喷发动机（WP8F = War Thunder Port 8111 Frontend，双关"涡喷-8F"）。

一处几何、两份产物 —— SVG 与 ICO 从同一组常量画出来，避免"改了图标忘了另一个"：

    gui/static/logo.svg      矢量（favicon + README 插图）
    gui/logo.ico             多尺寸（16/24/32/48/64/128/256），给 wp8f-gui.exe 与托盘

用法（需要 Pillow，Windows 侧 python 与 WSL 的 python3 都有）：

    python scripts/make_icon.py          # 重新生成上面三个文件
    python scripts/make_icon.py --check  # 只校验磁盘上的产物与本脚本一致（不写文件）

设计口径（改之前先想清楚，图标就这几件事）：

* **正面视图**：外圈机匣 + 8 片压气机叶片 + 轮毂 —— 16px 下也只剩这些能看清；
  8 片是 "8F" 的呼应（`WP8F` / 涡喷-8F）。
* **浅薄荷、无底透明**（用户口径：不要黑、也不要最外层那圈深薄荷圆底）：
  机匣环与轮毂用中浅薄荷（骨架），叶片用更淡的薄荷（填充）；
  透明底在深色任务栏/托盘上最清楚，浅色底上靠机匣环与轮毂的轮廓。
* **叶片前倾 12°**：暗示旋转方向，也让静止的图不呆板。
* 不写文字：16px 下任何字都是噪点。
"""

from __future__ import annotations

import argparse
import math
import struct
import sys
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# 产物都放 `gui/`：图标是**构建输入**（`gui/build.rs` + `tray.rs` 的 `include_bytes!`）
# 与前端资源（favicon / README 插图），必须随仓库走；
# `resource/` 只放本地资源（字体/语音/数据），不入库。
SVG_OUT = ROOT / "gui" / "static" / "logo.svg"   # 矢量源（favicon + README 插图）
ICO_OUT = ROOT / "gui" / "logo.ico"              # 多尺寸图标（exe 资源 + 托盘）

# ---------------------------------------------------------------- 几何/配色 --

VIEW = 256.0  # SVG 视图框边长（也是光栅化的基准尺寸）

# 配色：清新淡雅的浅薄荷 + 透明底（按用户口径：不要黑色，也不要最外层那圈深薄荷圆底）
CASING = (0x43, 0xD2, 0xA5, 0xFF)  # 机匣环 / 轮毂轮廓：中浅薄荷（图形骨架，浅底上也看得见）
BLADE = (0xA6, 0xEF, 0xD5, 0xFF)  # 叶片：更淡的薄荷

CASING_R = 100.0  # 机匣环中线半径
CASING_W = 16.0  # 机匣环线宽（16px 图标里环宽 ≈ 1px —— 再细就在下采样里糊掉了）
HUB_R = 26.0  # 轮毂中线半径
HUB_W = 15.0  # 轮毂线宽（画成圆环 → 中间是透明的进气口，不需要额外"挖洞"）
BLADE_IN = 34.0  # 叶片根部半径（= 轮毂外缘）
BLADE_OUT = CASING_R - CASING_W / 2.0 - 2.0  # 叶片尖部半径（顶到机匣内缘）
BLADE_N = 8  # 叶片数（"8F" 的 8）
BLADE_ROOT_HALF_DEG = 5.0  # 根部半宽（角度）
BLADE_TIP_HALF_DEG = 9.0  # 尖部半宽（角度）—— 尖宽根窄，才是压气机叶片的样子
BLADE_SWEEP_DEG = 12.0  # 尖部相对根部的旋转偏移（暗示转向）

ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)
SS = 4  # 每档尺寸的超采样倍数（先画大再缩，等于自己做抗锯齿）


@dataclass(frozen=True)
class Blade:
    """一片叶片：根部两点 + 尖部两点（角度制，0° = 正上方，顺时针）。"""

    root_a: tuple[float, float]
    root_b: tuple[float, float]
    tip_b: tuple[float, float]
    tip_a: tuple[float, float]


def polar(r: float, deg: float, cx: float = VIEW / 2, cy: float = VIEW / 2) -> tuple[float, float]:
    rad = math.radians(deg - 90.0)  # 0° 指向上方
    return (cx + r * math.cos(rad), cy + r * math.sin(rad))


def blades() -> list[Blade]:
    out: list[Blade] = []
    step = 360.0 / BLADE_N
    for i in range(BLADE_N):
        a = i * step
        out.append(
            Blade(
                root_a=polar(BLADE_IN, a - BLADE_ROOT_HALF_DEG),
                root_b=polar(BLADE_IN, a + BLADE_ROOT_HALF_DEG),
                tip_b=polar(BLADE_OUT, a + BLADE_SWEEP_DEG + BLADE_TIP_HALF_DEG),
                tip_a=polar(BLADE_OUT, a + BLADE_SWEEP_DEG - BLADE_TIP_HALF_DEG),
            )
        )
    return out


def hex_rgb(c: tuple[int, int, int, int]) -> str:
    return f"#{c[0]:02X}{c[1]:02X}{c[2]:02X}"


# ---------------------------------------------------------------------- SVG --


def svg_text() -> str:
    p = 2  # 坐标保留位数：几何算出来是浮点，SVG 里两位足够
    b = blades()
    polys = "\n".join(
        "    <polygon points=\"{}\" fill=\"{}\"/>".format(
            " ".join(f"{x:.{p}f},{y:.{p}f}" for x, y in (bl.root_a, bl.root_b, bl.tip_b, bl.tip_a)),
            hex_rgb(BLADE),
        )
        for bl in b
    )
    return f"""<?xml version="1.0" encoding="UTF-8"?>
<!-- wp8f logo：涡喷发动机正面（8 片压气机叶片 = WP8F / 涡喷-8F 的呼应）。
     无底透明：只有机匣环、叶片、轮毂三样，浅薄荷配色。
     本文件由 scripts/make_icon.py 生成 —— 改图形请改脚本里的几何常量再重新生成，
     直接手改这里会在下一次生成时被覆盖。 -->
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {VIEW:.0f} {VIEW:.0f}" width="{VIEW:.0f}" height="{VIEW:.0f}" role="img" aria-label="wp8f">
  <title>wp8f</title>
  <desc>War Thunder Port 8111 Frontend —— 涡喷发动机正面视图：机匣、8 片压气机叶片、轮毂。</desc>
  <circle cx="{VIEW / 2:.0f}" cy="{VIEW / 2:.0f}" r="{CASING_R:.0f}" fill="none" stroke="{hex_rgb(CASING)}" stroke-width="{CASING_W:.0f}"/>
{polys}
  <circle cx="{VIEW / 2:.0f}" cy="{VIEW / 2:.0f}" r="{HUB_R:.0f}" fill="none" stroke="{hex_rgb(CASING)}" stroke-width="{HUB_W:.0f}"/>
</svg>
"""


# -------------------------------------------------------------------- 光栅化 --


def render(size: int):
    """画一张 size×size 的 RGBA（内部按 SS 倍超采样）。"""
    from PIL import Image, ImageDraw  # 延迟导入：--check 与只看用法时不强求 Pillow

    s = size * SS
    k = s / VIEW
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    c = s / 2

    def scaled(pts):
        return [(x * k, y * k) for x, y in pts]

    # 机匣环与轮毂都画成"圆环"：外圆填色 + 内圆抹回透明（无底色，抹回去就是透出背景）
    for r_mid, w in ((CASING_R, CASING_W), (HUB_R, HUB_W)):
        r_out, r_in = (r_mid + w / 2) * k, (r_mid - w / 2) * k
        d.ellipse([c - r_out, c - r_out, c + r_out, c + r_out], fill=CASING)
        d.ellipse([c - r_in, c - r_in, c + r_in, c + r_in], fill=(0, 0, 0, 0))
    for bl in blades():
        d.polygon(scaled((bl.root_a, bl.root_b, bl.tip_b, bl.tip_a)), fill=BLADE)
    return img.resize((size, size), Image.LANCZOS)


def png_bytes(img) -> bytes:
    buf = BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def ico_bytes() -> bytes:
    """手写 ICO 容器：每个尺寸一条 PNG 记录（Vista+ 支持，体积比 BMP 小一个数量级）。

    布局：ICONDIR(6B) + N × ICONDIRENTRY(16B) + N 段 PNG。
    256 的宽高字段写 0（这是 ICO 格式对 256 的规定）。
    """
    entries, blobs = [], []
    offset = 6 + 16 * len(ICO_SIZES)
    for size in ICO_SIZES:
        data = png_bytes(render(size))
        entries.append(
            struct.pack(
                "<BBBBHHII",
                0 if size >= 256 else size,
                0 if size >= 256 else size,
                0,  # 调色板色数（真彩 = 0）
                0,  # 保留
                1,  # 色彩平面
                32,  # 位深
                len(data),
                offset,
            )
        )
        blobs.append(data)
        offset += len(data)
    return struct.pack("<HHH", 0, 1, len(ICO_SIZES)) + b"".join(entries) + b"".join(blobs)


# ---------------------------------------------------------------------- CLI --


def main() -> int:
    ap = argparse.ArgumentParser(description="生成 wp8f logo（SVG + 多尺寸 ICO）")
    ap.add_argument("--check", action="store_true", help="只校验产物是否与脚本一致，不写文件")
    args = ap.parse_args()

    svg = svg_text().encode("utf-8")
    if args.check:
        want = {SVG_OUT: svg}
        bad = [p for p, data in want.items() if not p.exists() or p.read_bytes() != data]
        if bad or not ICO_OUT.exists():
            for p in bad:
                print(f"[!!] {p.relative_to(ROOT)} 与脚本不一致（跑 python scripts/make_icon.py 重新生成）")
            if not ICO_OUT.exists():
                print(f"[!!] {ICO_OUT.relative_to(ROOT)} 不存在")
            return 1
        from PIL import Image

        with Image.open(ICO_OUT) as im:
            sizes = sorted(im.info.get("sizes", []))
        print(f"[ok] logo 产物一致：{SVG_OUT.name} / {ICO_OUT.name} {sizes}")
        return 0

    try:
        ico = ico_bytes()
    except ImportError:
        print("[!!] 需要 Pillow：pip install pillow（SVG 不依赖它，ICO 与 --check 需要）", file=sys.stderr)
        return 2

    SVG_OUT.parent.mkdir(parents=True, exist_ok=True)
    SVG_OUT.write_bytes(svg)
    ICO_OUT.write_bytes(ico)
    print(f"[ok] {SVG_OUT.relative_to(ROOT)}   {len(svg)} B（favicon + README 插图）")
    print(f"[ok] {ICO_OUT.relative_to(ROOT)}   {len(ico)} B  {list(ICO_SIZES)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
