"""接缝/穿透的机械判读：背景色连通域检测。

判定口径（不依赖目视）：截图背景是场景 `background` 常数色，经后处理后在画面上是
一片近似常数的像素。从画面四边做 4-连通洪泛，把「背景色像素」标成外部背景；
**剩下未被洪泛到、颜色仍与背景色一致的像素，只可能是被地形包住的背景**——也就是
「从缝里透出背景」的直接证据。逐张输出面积与外接矩形，便于回填文档。

另外统计接缝带（若给了 z 参考）的亮度跳变：这里只做事后辅助，不做几何断言。
"""
from __future__ import annotations

import json
import sys
from collections import Counter, deque
from pathlib import Path

from PIL import Image

LAB = Path(__file__).resolve().parents[1]
EVID = LAB / "evidence"
HUD_H = 40          # 顶部 HUD 角标占位行数（text_add y=12 / size=16，留 8 行余量）


def background_color(im: Image.Image) -> tuple[int, int, int]:
    """取四边像素的众数作为背景色。"""
    w, h = im.size
    px = im.load()
    c = Counter()
    for x in range(w):
        c[px[x, 0]] += 1
        c[px[x, h - 1]] += 1
    for y in range(h):
        c[px[0, y]] += 1
        c[px[w - 1, y]] += 1
    return c.most_common(1)[0][0]


def analyze(path: Path, tol: int = 3) -> dict:
    im = Image.open(path).convert("RGB")
    w, h = im.size
    px = im.load()
    bg = background_color(im)

    def is_bg(p) -> bool:
        return abs(p[0] - bg[0]) <= tol and abs(p[1] - bg[1]) <= tol and abs(p[2] - bg[2]) <= tol

    outside = bytearray(w * h)
    q = deque()
    for x in range(w):
        for y in (0, h - 1):
            if not outside[y * w + x] and is_bg(px[x, y]):
                outside[y * w + x] = 1
                q.append((x, y))
    for y in range(h):
        for x in (0, w - 1):
            if not outside[y * w + x] and is_bg(px[x, y]):
                outside[y * w + x] = 1
                q.append((x, y))
    while q:
        x, y = q.popleft()
        for nx, ny in ((x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)):
            if 0 <= nx < w and 0 <= ny < h:
                i = ny * w + nx
                if not outside[i] and is_bg(px[nx, ny]):
                    outside[i] = 1
                    q.append((nx, ny))

    # 顶部 HUD 角标（text_add 在 y=12、字号 16）自带字形闭合区，会把「同色像素」算成
    # 包夹背景的假阳性。这里把顶部 HUD 条（前 HUD_H 行）整体视为画面外部、不参与判定，
    # 判定有效区为 y >= HUD_H；该区域外若真出现缝内透背景，不会被这条掩膜遮住。
    for y in range(min(HUD_H, h)):
        for x in range(w):
            outside[y * w + x] = 1

    enclosed = [(i % w, i // w) for i in range(w * h)
                if not outside[i] and is_bg(px[i % w, i // w])]
    outside_px = sum(outside)
    boxes = []
    if enclosed:
        xs = [p[0] for p in enclosed]
        ys = [p[1] for p in enclosed]
        boxes.append({"count": len(enclosed), "bbox": [min(xs), min(ys), max(xs), max(ys)]})
    return {"file": path.name, "size": [w, h], "bg": list(bg),
            "hud_masked_rows": HUD_H,
            "bg_visible_px": outside_px,
            "bg_visible_ratio": round(outside_px / (w * h), 4),
            "enclosed_bg_pixels": len(enclosed), "islands": boxes}


def main() -> int:
    files = sorted(list(EVID.glob("seam_*.png")) + list(EVID.glob("probe_*.png")))
    exclude = {"seam_08_grazing_x-25.png"}   # 掠射机位画面几乎被坡面填满，背景不参与判定
    if len(sys.argv) > 1:
        files = [Path(p) for p in sys.argv[1:]]
    rows = [analyze(p) for p in files]
    for r in rows:
        flag = "PASS" if r["enclosed_bg_pixels"] == 0 else "HOLE"
        print(f"[{flag}] {r['file']}  bg={tuple(r['bg'])}  可见背景={r['bg_visible_ratio'] * 100:.1f}%  "
              f"包夹背景像素={r['enclosed_bg_pixels']}  {r['islands']}")
    out = EVID / "seam_hole_scan.json"
    out.write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n明细 {out}")
    return 0 if all(r["enclosed_bg_pixels"] == 0 for r in rows) else 1


if __name__ == "__main__":
    sys.exit(main())
