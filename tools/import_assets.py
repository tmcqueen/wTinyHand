#!/usr/bin/env python3
"""Import hand art from the upstream macOS TinyHand checkout.

Resizes each hand, measures where the hand/rod/handle are (same algorithm as
upstream's HandAsset.measure), and pre-renders thumbnails and soft shadows so
the app itself only needs PyQt6.

    tools/import_assets.py ../tinyhand
"""
import json
import shutil
import sys
from pathlib import Path

from PIL import Image, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "assets"
HEIGHT = 768
SHADOW_DIV = 4      # shadow is stored at 1/4 of the hand resolution
SHADOW_PAD = 16     # transparent margin around the shadow, in shadow pixels
SHADOW_SIGMA = 5


def measure(img):
    w, h = img.size
    data = img.getchannel("A").point(lambda a: 255 if a > 128 else 0).tobytes()
    rows = [data[y * w:(y + 1) * w] for y in range(h)]
    counts = [w - r.count(0) for r in rows]

    def center(y):
        lo, hi = rows[y].find(b"\xff"), rows[y].rfind(b"\xff")
        return 0.5 if lo < 0 else (lo + hi) / 2 / w

    top = next(y for y in range(h) if counts[y] > 0)
    max_w = max(counts[top:min(h, top + int(0.45 * h))])
    fist = next(y for y in range(top, h) if counts[y] > 0.6 * max_w)
    wrist = next(y for y in range(fist, h) if counts[y] < 0.35 * max_w)
    rod_w = counts[min(h - 1, wrist + h // 50)]
    last = max(y for y in range(h) if counts[y] > 0)
    start = min(h - 1, wrist + h // 33)
    rod_end = next((y for y in range(start, h) if counts[y] > 1.8 * rod_w), last)
    xs = [(r.find(b"\xff"), r.rfind(b"\xff")) for r in rows[top:wrist] if r.count(0) < w]
    return {
        "aspect": w / h,
        "handTop": max(0, top - 0.004 * h) / h,
        "tip": top / h,
        "wrist": wrist / h,
        "rodEnd": rod_end / h,
        "tipX": center(min(h - 1, top + h // 200)),
        "rodX": center((wrist + rod_end) // 2),
        "handMinX": min(x[0] for x in xs) / w,
        "handMaxX": max(x[1] for x in xs) / w,
    }


def thumbnail(img, m):
    w, h = img.size
    side = max((m["handMaxX"] - m["handMinX"]) * w, (m["wrist"] - m["handTop"]) * h) * 1.06
    cx, cy = (m["handMinX"] + m["handMaxX"]) / 2 * w, (m["handTop"] + m["wrist"]) / 2 * h
    box = tuple(round(v) for v in (cx - side / 2, cy - side / 2, cx + side / 2, cy + side / 2))
    return img.crop(box).resize((128, 128), Image.LANCZOS)


def shadow(img):
    w, h = img.size
    a = img.getchannel("A").resize((w // SHADOW_DIV, h // SHADOW_DIV), Image.LANCZOS)
    canvas = Image.new("L", (a.width + 2 * SHADOW_PAD, a.height + 2 * SHADOW_PAD), 0)
    canvas.paste(a, (SHADOW_PAD, SHADOW_PAD))
    canvas = canvas.filter(ImageFilter.GaussianBlur(SHADOW_SIGMA))
    out = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    out.putalpha(canvas)
    return out


def main():
    src = Path(sys.argv[1] if len(sys.argv) > 1 else "../tinyhand").resolve()
    for d in ("hands", "thumbs", "shadows"):
        (OUT / d).mkdir(parents=True, exist_ok=True)
    metrics = {}
    for path in sorted((src / "assets").glob("hand_*.png")):
        name = path.stem[len("hand_"):]
        full = Image.open(path).convert("RGBA")
        m = measure(full)
        small = full.resize((round(HEIGHT * full.width / full.height), HEIGHT), Image.LANCZOS)
        small.save(OUT / "hands" / f"{name}.png", optimize=True)
        thumbnail(full, m).save(OUT / "thumbs" / f"{name}.png", optimize=True)
        shadow(small).save(OUT / "shadows" / f"{name}.png", optimize=True)
        metrics[name] = {k: round(v, 5) for k, v in m.items()}
        print(name, metrics[name])
    meta = {"shadowDiv": SHADOW_DIV, "shadowPad": SHADOW_PAD, "hands": metrics}
    (OUT / "hands.json").write_text(json.dumps(meta, indent=1, sort_keys=True) + "\n")
    shutil.copy(src / "docs/assets/tap.wav", OUT / "tap.wav")
    shutil.copy(src / "docs/assets/mark-512.png", OUT / "icon.png")
    for n in ("menubar_on@2x.png", "menubar_off@2x.png"):
        shutil.copy(src / "assets" / n, OUT / n.replace("@2x", ""))
    shutil.copy(src / "LICENSE", OUT.parent / "LICENSE")


if __name__ == "__main__":
    main()
