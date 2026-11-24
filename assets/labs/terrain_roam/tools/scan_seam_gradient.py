"""接缝带的定量判读：把世界坐标 z=-50 的接缝线投影到截图，沿接缝带量亮度梯度。

判定口径：接缝两侧若存在几何错位或法线突变，接缝处会出现一条窄的亮度跳变带，
表现为「垂直方向亮度梯度」的尖峰。做法：

1. 按 `Camera` 的口径（rh lookAt + directx 透视、垂直 fov、aspect=16/9）把接缝折线
   `(x, height(x), z=-50)` 投影到像素坐标；
2. 逐列在接缝行 ±4px 内取垂直梯度峰值 `peak`；
3. 同列在离接缝 30~50px 的行带上取同一梯度统计作为该列的「同材质对照」`ref`；
4. 报 `peak` 的中位数 / 95 分位、`peak - ref` 的中位数，以及全图最大梯度位置。

若接缝带真的和周围一样平（peak≈ref），说明接缝在画面上没有台阶；反之 peak 明显高于
ref，就得回到原图确认是不是错位。
"""
from __future__ import annotations

import json
import math
import sys
from pathlib import Path

import numpy as np
from PIL import Image

LAB = Path(__file__).resolve().parents[1]
EVID = LAB / "evidence"
MANIFEST = EVID / "seam_manifest.json"
GROUND_KNOTS = [(-100.0, 0.0), (-60.0, 0.0), (-40.0, 5.358), (-20.0, 29.193), (100.0, 29.193)]


def ground(x: float) -> float:
    for (x0, y0), (x1, y1) in zip(GROUND_KNOTS, GROUND_KNOTS[1:]):
        if x0 <= x <= x1:
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    return GROUND_KNOTS[-1][1]


def look_at(eye, target, up=(0.0, 1.0, 0.0)) -> np.ndarray:
    eye = np.asarray(eye, dtype=np.float64)
    f = np.asarray(target, dtype=np.float64) - eye
    f /= np.linalg.norm(f)
    up = np.asarray(up, dtype=np.float64)
    s = np.cross(f, up)
    s /= np.linalg.norm(s)
    u = np.cross(s, f)
    m = np.eye(4)
    m[0, :3], m[1, :3], m[2, :3] = s, u, -f
    m[0, 3], m[1, 3], m[2, 3] = -s @ eye, -u @ eye, f @ eye
    return m


def perspective(fov_deg: float, aspect: float, near: float = 0.05, far: float = 500.0) -> np.ndarray:
    t = math.tan(math.radians(fov_deg) * 0.5)
    m = np.zeros((4, 4))
    m[0, 0] = 1.0 / (aspect * t)
    m[1, 1] = 1.0 / t
    m[2, 2] = far / (near - far)
    m[2, 3] = far * near / (near - far)
    m[3, 2] = -1.0
    return m


def project(vp: np.ndarray, p, w: int, h: int):
    v = vp @ np.array([p[0], p[1], p[2], 1.0])
    if v[3] <= 1e-6:
        return None
    ndc = v[:3] / v[3]
    if not (0.0 <= ndc[2] <= 1.0):
        return None
    return ((ndc[0] * 0.5 + 0.5) * w, (1.0 - (ndc[1] * 0.5 + 0.5)) * h)


def bg_color(arr: np.ndarray, tol: int = 3):
    """四边像素众数 = 清屏底色（与 scan_seam_holes.py 同口径）。"""
    edges = np.concatenate([arr[0, :, :], arr[-1, :, :], arr[:, 0, :], arr[:, -1, :]])
    vals, counts = np.unique(edges.astype(np.int32), axis=0, return_counts=True)
    return vals[counts.argmax()]


def analyze(st: dict) -> dict:
    path = EVID / st["file"]
    im = Image.open(path).convert("RGB")
    arr = np.asarray(im, dtype=np.float64)
    h, w = arr.shape[:2]
    lum = arr[..., 0] * 0.299 + arr[..., 1] * 0.587 + arr[..., 2] * 0.114
    bg = bg_color(arr)
    vp = perspective(st["fov"], w / h) @ look_at(st["eye"], st["target"])

    seam_px = []
    for i in range(-100, 101):
        p = project(vp, (float(i), ground(float(i)), -50.0), w, h)
        if p and 2 <= p[0] < w - 2 and 2 <= p[1] < h - 2:
            seam_px.append(p)
    if len(seam_px) < 6:
        return {"file": st["file"], "skipped": "接缝不在画面内或投影点过少", "seam_points": len(seam_px)}

    peaks, refs, silhouette_cols = [], [], 0
    for x, y in seam_px:
        col = int(round(x))
        yc = int(round(y))
        lo, hi = max(1, yc - 3), min(h - 1, yc + 4)
        window = arr[lo - 1:hi + 1, col]
        # 掠影列：接缝带窗口里混进了清屏底色（地形与天空的交界），梯度来自轮廓而非接缝，
        # 这类列不计入统计，否则会被轮廓边当成「台阶」假阳性。
        if np.any(np.all(np.abs(window - bg) <= 3, axis=1)):
            silhouette_cols += 1
            continue
        g = np.abs(np.diff(lum[lo - 1:hi + 1, col]))
        peak = float(g.max()) if g.size else 0.0
        # 同列对照：离接缝 30~50px 的行带（同材质、无接缝）
        gl = np.abs(np.diff(lum[max(1, yc - 50):max(2, yc - 30), col]))
        gr = np.abs(np.diff(lum[min(h - 2, yc + 30):min(h - 1, yc + 50), col]))
        ref_pool = np.concatenate([gl, gr]) if (gl.size + gr.size) else np.zeros(1)
        peaks.append(peak)
        refs.append(float(ref_pool.max()))
    if not peaks:
        return {"file": st["file"], "skipped": "所有接缝列都落在轮廓边上", "seam_points": len(seam_px)}
    peaks = np.array(peaks)
    refs = np.array(refs)
    seam_rows = [p[1] for p in seam_px]

    # 该机位在接缝处的「地面可分辨尺度」：一行像素对应的地面深度（米/像素）。
    # 台阶只要达到约 1 像素高度就会被本统计抓到，故这个值同时是本次判读的分辨率下限。
    eye = np.asarray(st["eye"], dtype=np.float64)
    mpp = []
    for i in range(-100, 101, 4):
        dist = float(np.linalg.norm(np.array([float(i), ground(float(i)), -50.0]) - eye))
        mpp.append(dist * 2.0 * math.tan(math.radians(st["fov"]) * 0.5) / h)
    return {
        "file": st["file"],
        "seam_points": len(seam_px),
        "cols_used": int(peaks.size),
        "cols_skipped_silhouette": silhouette_cols,
        "seam_row_range": [round(min(seam_rows), 1), round(max(seam_rows), 1)],
        "peak_median": round(float(np.median(peaks)), 2),
        "peak_p95": round(float(np.percentile(peaks, 95)), 2),
        "peak_max": round(float(peaks.max()), 2),
        "ref_median": round(float(np.median(refs)), 2),
        "diff_median": round(float(np.median(peaks - refs)), 2),
        "flat_col_ratio": round(float(np.mean(peaks <= 2.0)), 3),
        "m_per_px_median": round(float(np.median(mpp)), 3),
        "m_per_px_range": [round(min(mpp), 3), round(max(mpp), 3)],
    }


def main() -> int:
    data = json.loads(MANIFEST.read_text(encoding="utf-8"))
    rows = []
    for st in data["stations"]:
        if st["name"].startswith("probe_"):
            continue          # 探针图底色被改成洋红，亮度梯度不具可比性，不参与此项统计
        r = analyze(st)
        rows.append(r)
        if "skipped" in r:
            print(f"[SKIP] {r['file']}: {r['skipped']} ({r['seam_points']} 点)")
        else:
            print(f"[{r['file']}] 接缝行 {r['seam_row_range']} / 有效列 {r['cols_used']}"
                  f"（轮廓列剔除 {r['cols_skipped_silhouette']}） | "
                  f"peak 中位 {r['peak_median']} p95 {r['peak_p95']} max {r['peak_max']} | "
                  f"同材质对照 {r['ref_median']} | 差值中位 {r['diff_median']} | "
                  f"平整列占比 {r['flat_col_ratio'] * 100:.0f}% | "
                  f"地面像素尺度 {r['m_per_px_median']} m/px（仅几何换算式，可辨台阶还受着色斜率限制，见 M2_BASELINE §7.4）")
    out = EVID / "seam_gradient_scan.json"
    out.write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n明细 {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
