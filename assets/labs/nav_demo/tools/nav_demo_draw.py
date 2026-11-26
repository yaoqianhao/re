"""M2-D07 导航寻路演示 —— 步骤二：出图。

输入 evidence/data.json + evidence/base_overview.png + evidence/step_*.png，
输出三张图：
  nav_demo_path.png     引擎实拍俯视图 + 红色寻路路线（主交付）
  nav_demo_profile.png  地形剖面（说明"有坡度"：平地 / 13.1° 缓坡 / 21.8° 陡坡 / 平台）
  nav_demo_steps.png    8 帧过程拼图（圆柱体 agent 从起点走到终点）

世界坐标 -> 屏幕像素直接用 data.json 里标定过的投影矩阵，实测残差 < 1px。
"""
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

LAB = Path(r"D:\rust3d-engine\assets\labs\nav_demo")
EV = LAB / "evidence"
SCENE = LAB / "nav_demo.r3e.json"

RED = (235, 30, 30)
RED_DARK = (90, 0, 0)
GREEN = (30, 220, 70)
BLUE = (50, 120, 255)
ORANGE = (250, 145, 50)
YELLOW = (255, 214, 60)
WHITE = (255, 255, 255)
BLACK = (0, 0, 0)


def font(size, bold=False):
    for name in (("msyhbd.ttc" if bold else "msyh.ttc"), "simhei.ttf", "simsun.ttc"):
        p = Path(r"C:\Windows\Fonts") / name
        if p.exists():
            try:
                return ImageFont.truetype(str(p), size)
            except Exception:
                pass
    return ImageFont.load_default()


def project(P, pts, w, h):
    pts = np.atleast_2d(np.array(pts, dtype=float))
    hh = np.hstack([pts, np.ones((len(pts), 1))]) @ P.T
    ndc = hh[:, :2] / hh[:, 2:3]
    return np.stack([(ndc[:, 0] + 1.0) * 0.5 * w, (1.0 - ndc[:, 1]) * 0.5 * h], axis=1)


def text_box(d, xy, s, f, fill=WHITE, anchor="la", outline=True):
    x, y = xy
    if outline:
        for dx, dy in ((-2, 0), (2, 0), (0, -2), (0, 2), (-1, -1), (1, 1), (-1, 1), (1, -1)):
            d.text((x + dx, y + dy), s, font=f, fill=BLACK, anchor=anchor)
    d.text((x, y), s, font=f, fill=fill, anchor=anchor)


def draw_path():
    data = json.loads((EV / "data.json").read_text(encoding="utf-8"))
    W = data["render"]["width"]
    H = data["render"]["height"]
    P = np.array(data["projection"], dtype=float).reshape(3, 4)
    scene = json.loads(SCENE.read_text(encoding="utf-8"))

    img = Image.open(EV / data["base_png"]).convert("RGB").resize((W, H))
    d = ImageDraw.Draw(img, "RGBA")
    f_title = font(26, True)
    f_lab = font(19, True)
    f_small = font(15)

    # ---- 障碍线框（用场景里的 AABB 投影） ----
    ob_of_interest = ["wall_a", "wall_b", "pillar_a", "pillar_b"]
    for ob in scene["objects"]:
        if ob["name"] not in ob_of_interest:
            continue
        cx, cy, cz = ob["position"]
        hx, hy, hz = [s / 2 for s in ob["scale"]]
        corners = [[cx + sx * hx, cy + sy * hy, cz + sz * hz]
                   for sx in (-1, 1) for sy in (-1, 1) for sz in (-1, 1)]
        scr = project(P, corners, W, H)
        idx = {(sx, sy, sz): i for i, (sx, sy, sz) in enumerate(
            [(a, b, c) for a in (-1, 1) for b in (-1, 1) for c in (-1, 1)])}
        edges = []
        for sx in (-1, 1):
            for sy in (-1, 1):
                edges.append(((sx, sy, -1), (sx, sy, 1)))
        for sx in (-1, 1):
            for sz in (-1, 1):
                edges.append(((sx, -1, sz), (sx, 1, sz)))
        for sy in (-1, 1):
            for sz in (-1, 1):
                edges.append(((-1, sy, sz), (1, sy, sz)))
        for a, b in edges:
            d.line([tuple(scr[idx[a]]), tuple(scr[idx[b]])], fill=(255, 235, 120, 210), width=2)
        top = project(P, [[cx, cy + hy, cz]], W, H)[0]
        label = {"wall_a": "墙 wall_a", "wall_b": "墙 wall_b",
                 "pillar_a": "柱 pillar_a", "pillar_b": "柱 pillar_b"}[ob["name"]]
        text_box(d, (top[0], top[1] - 26), label, f_small, fill=YELLOW, anchor="ma")

    # ---- 原始 A* 折线（nav.path 输出，灰白虚线） ----
    raw = project(P, data["raw_points"], W, H)
    for i in range(len(raw) - 1):
        x0, y0 = raw[i]
        x1, y1 = raw[i + 1]
        n = max(2, int(math.dist((x0, y0), (x1, y1)) / 12))
        for k in range(n):
            if k % 2:
                continue
            a = k / n
            b = (k + 1) / n
            d.line([(x0 + (x1 - x0) * a, y0 + (y1 - y0) * a),
                    (x0 + (x1 - x0) * b, y0 + (y1 - y0) * b)], fill=(255, 255, 255, 170), width=2)

    # ---- 平滑后的红色路线（主视觉） ----
    sm = project(P, data["smooth_points"], W, H)
    pts = [tuple(p) for p in sm]
    d.line(pts, fill=(20, 0, 0, 220), width=9, joint="curve")
    d.line(pts, fill=RED, width=5, joint="curve")
    for p in pts:  # 折点标记
        d.ellipse([p[0] - 5, p[1] - 5, p[0] + 5, p[1] + 5], fill=(255, 255, 255), outline=RED_DARK)

    # ---- 过程采样点 ----
    steps = project(P, [s for s in data["steps"]], W, H)
    for k, (x, y) in enumerate(steps):
        d.ellipse([x - 7, y - 7, x + 7, y + 7], fill=(255, 255, 255, 235), outline=RED, width=3)
        text_box(d, (x + 11, y + 8), f"t{k}", f_small, fill=WHITE)

    # ---- 起点 / 终点 ----
    s_scr = project(P, [data["start"]], W, H)[0]
    g_scr = project(P, [data["goal"]], W, H)[0]
    for scr, col, lab, off in ((s_scr, GREEN, "起点 START", (-16, 22)), (g_scr, BLUE, "终点 GOAL", (16, 22))):
        d.ellipse([scr[0] - 11, scr[1] - 11, scr[0] + 11, scr[1] + 11], fill=col, outline=BLACK, width=2)
        text_box(d, (scr[0] + off[0], scr[1] + off[1]), lab, f_lab, fill=col, anchor="ma")

    # ---- 坡度 / 地貌标注 ----
    knots = data["terrain_profile"]
    hgt = lambda x: (lambda k: (k[0][1] if x <= k[0][0] else next(
        (y0 + (y1 - y0) * (x - x0) / (x1 - x0) for (x0, y0), (x1, y1) in zip(k, k[1:]) if x0 <= x <= x1),
        k[-1][1])))(knots)
    for x, z, lab in ((-48, 6, "平地 h=0"), (-20, 14, "缓坡 13.1°"), (10, -16, "陡坡 21.8°"), (40, 15, "平台 h=19m")):
        p = project(P, [[x, hgt(x) + 0.4, z]], W, H)[0]
        text_box(d, tuple(p), lab, f_small, fill=(235, 240, 255), anchor="ma")

    # ---- 标题条 ----
    d.rectangle([0, 0, W, 54], fill=(0, 0, 0, 165))
    text_box(d, (16, 13), "r3e 导航寻路演示：圆柱体从起点绕过障碍走到终点（红线 = nav.path + nav.smooth 平滑后的实际路线）",
             f_title, fill=WHITE, outline=False)

    stats = data["stats"]
    bake = data["bake"]
    foot = (f"cell {bake['cellSize']}m  |  walkable {bake['walkable']} / {bake['cells']} 格 "
            f"({bake['walkableRatio'] * 100:.1f}%)  |  路径 {path_len(data):.1f}m  |  "
            f"{len(data['raw_points'])} 个 waypoint  ->  smooth {len(data['smooth_points'])} 点  |  "
            f"坡度分布 flat {stats['classHistogram']['flat']} / slope {stats['classHistogram']['slope']} "
            f"/ blocked {stats['classHistogram']['blocked']}")
    d.rectangle([0, H - 34, W, H], fill=(0, 0, 0, 175))
    text_box(d, (16, H - 27), foot, f_small, fill=(230, 230, 230), outline=False)

    out = LAB / "nav_demo_path.png"
    img.save(out)
    print("wrote", out, img.size)
    return out


def path_len(data):
    pts = np.array(data["smooth_points"], dtype=float)
    return float(np.linalg.norm(np.diff(pts[:, [0, 2]], axis=0), axis=1).sum())


def draw_profile():
    data = json.loads((EV / "data.json").read_text(encoding="utf-8"))
    knots = data["terrain_profile"]
    Wd, Ht = 1180, 470
    mL, mR, mT, mB = 70, 40, 60, 70
    img = Image.new("RGB", (Wd, Ht), (20, 22, 26))
    d = ImageDraw.Draw(img)
    f_title = font(24, True)
    f_lab = font(17, True)
    f_small = font(14)
    d.text((16, 14), "地形剖面（沿 +X）：平地 -> 13.1° 缓坡 -> 21.8° 陡坡 -> 平台，障碍全部落在坡面上",
           font=f_title, fill=WHITE)

    X0, X1 = -60.0, 60.0
    Y0, Y1 = 0.0, 26.0
    px = lambda x: mL + (x - X0) / (X1 - X0) * (Wd - mL - mR)
    py = lambda y: Ht - mB - (y - Y0) / (Y1 - Y0) * (Ht - mT - mB)

    def h(x):
        if x <= knots[0][0]:
            return knots[0][1]
        for (x0, y0), (x1, y1) in zip(knots, knots[1:]):
            if x0 <= x <= x1:
                return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        return knots[-1][1]

    zones = [(-60, -35, (52, 60, 72), "平地 0°"), (-35, -5, (120, 96, 40), "缓坡 13.1°"),
             (-5, 25, (140, 68, 40), "陡坡 21.8°"), (25, 60, (52, 78, 58), "平台 0°")]
    poly = [(px(X0), py(0))]
    for x in np.linspace(X0, X1, 240):
        poly.append((px(x), py(h(x))))
    poly.append((px(X1), py(0)))
    d.polygon(poly, fill=(70, 76, 70))
    for x0, x1, col, lab in zones:
        seg = [(px(x0), py(0))] + [(px(x), py(h(x))) for x in np.linspace(x0, x1, 60)] + [(px(x1), py(0))]
        d.polygon(seg, fill=col)
        cx = px((x0 + x1) / 2)
        d.text((cx, py(h((x0 + x1) / 2)) - 26), lab, font=f_lab, fill=WHITE, anchor="mb")
        d.line([(px(x0), py(0)), (px(x0), py(24))], fill=(120, 120, 120), width=1)

    # 轴线
    d.line([(mL, py(0)), (Wd - mR, py(0))], fill=(160, 160, 160), width=2)
    for y in (0, 5, 10, 15, 20, 25):
        d.line([(mL - 6, py(y)), (mL, py(y))], fill=(160, 160, 160), width=1)
        d.text((mL - 10, py(y)), f"{y}m", font=f_small, fill=(190, 190, 190), anchor="rm")
    for x in range(-60, 61, 20):
        d.text((px(x), py(0) + 8), f"x={x}", font=f_small, fill=(190, 190, 190), anchor="ma")

    # 障碍落位
    obstacles = [("wall_a", 4, 7, YELLOW), ("pillar_a", 7.7, 10.7, ORANGE),
                 ("pillar_b", 12.3, 15.3, ORANGE), ("wall_b", 16, 19, YELLOW)]
    for name, xa, xb, col in obstacles:
        d.rectangle([px(xa), py(h(xa)), px(xb), py(0)], outline=col, width=2)
        d.text((px((xa + xb) / 2), py(h(xa)) - 6), name, font=f_small, fill=col, anchor="mb")

    # 起终点
    sx = px(-50)
    d.line([(sx, py(h(-50)) - 46), (sx, py(h(-50)) - 8)], fill=GREEN, width=3)
    d.text((sx, py(h(-50)) - 52), "起点 x=-50", font=f_lab, fill=GREEN, anchor="mb")
    gx = px(48)
    d.line([(gx, py(h(48)) - 46), (gx, py(h(48)) - 8)], fill=BLUE, width=3)
    d.text((gx, py(h(48)) - 52), "终点 x=48", font=f_lab, fill=BLUE, anchor="mb")

    d.text((16, Ht - 30), "水平面 120m x 50m；坡面高度取自场景 terrain.heights（25x13 顶点，5.0m x 4.17m 网格）",
           font=f_small, fill=(180, 180, 180))
    out = LAB / "nav_demo_profile.png"
    img.save(out)
    print("wrote", out, img.size)
    return out


def draw_steps():
    data = json.loads((EV / "data.json").read_text(encoding="utf-8"))
    all_frames = data["frames"]
    if len(all_frames) > 8:
        idx = [round(i * (len(all_frames) - 1) / 7) for i in range(8)]
        frames = [all_frames[i] for i in idx]
    else:
        frames = all_frames
    cols = 4
    rows = math.ceil(len(frames) / cols)
    tw, th = 640, 400
    gap = 10
    pad = 16
    Wd = pad * 2 + cols * tw + (cols - 1) * gap
    Ht = pad * 2 + 54 + rows * (th + 26) + (rows - 1) * gap
    img = Image.new("RGB", (Wd, Ht), (18, 20, 24))
    d = ImageDraw.Draw(img)
    f_title = font(24, True)
    f_lab = font(15, True)
    d.text((pad, pad), "过程帧：圆柱体 agent 沿平滑路线逐点前进（全程 %d 帧，这里等间隔取 8 帧）" % len(all_frames),
           font=f_title, fill=WHITE)

    knots = data["terrain_profile"]

    def h(x):
        if x <= knots[0][0]:
            return knots[0][1]
        for (x0, y0), (x1, y1) in zip(knots, knots[1:]):
            if x0 <= x <= x1:
                return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
        return knots[-1][1]

    for i, fr in enumerate(frames):
        r, c = divmod(i, cols)
        x0 = pad + c * (tw + gap)
        y0 = pad + 54 + r * (th + 26 + gap)
        im = Image.open(EV / fr["png"]).convert("RGB").resize((tw, th))
        img.paste(im, (x0, y0))
        p = fr["point"]
        hx = p[0]
        a = math.degrees(math.atan((h(hx + 0.5) - h(hx - 0.5)) / 1.0))
        d.text((x0 + 6, y0 + th + 5),
               f"frame {fr['k']}  ({p[0]:.1f}, {p[1]:.1f}, {p[2]:.1f})   地面高 {h(hx):.1f}m  坡度 {abs(a):.1f}°",
               font=f_lab, fill=(220, 220, 220))
        d.rectangle([x0, y0, x0 + tw - 1, y0 + th - 1], outline=(70, 70, 70))
    out = LAB / "nav_demo_steps.png"
    img.save(out)
    print("wrote", out, img.size)
    return out


def draw_gif():
    """把全部过程帧合成 GIF：红线随圆柱体推进而增长。"""
    data = json.loads((EV / "data.json").read_text(encoding="utf-8"))
    frames = data["frames"]
    if len(frames) < 3:
        return None
    W = data["render"]["width"]
    H = data["render"]["height"]
    P = np.array(data["projection"], dtype=float).reshape(3, 4)
    W2, H2 = 832, 520
    sx, sy = W2 / W, H2 / H
    f_lab = font(20, True)
    step_pts = project(P, data["steps"], W, H) * np.array([sx, sy])
    imgs = []
    for fr in frames:
        k = fr["k"]
        im = Image.open(EV / fr["png"]).convert("RGB").resize((W2, H2))
        d = ImageDraw.Draw(im, "RGBA")
        pts = [tuple(p) for p in step_pts[: k + 1]]
        if len(pts) >= 2:
            d.line(pts, fill=(20, 0, 0, 210), width=7, joint="curve")
            d.line(pts, fill=RED, width=4, joint="curve")
        cx, cy = step_pts[k]
        d.ellipse([cx - 9, cy - 9, cx + 9, cy + 9], fill=(255, 255, 255, 235), outline=RED, width=3)
        prog = 100.0 * k / (len(frames) - 1)
        d.rectangle([0, 0, W2, 34], fill=(0, 0, 0, 150))
        d.text((10, 7), f"frame {k:02d}/{len(frames) - 1:02d}   进度 {prog:5.1f}%   "
                        f"地面高 {fr['point'][1] - 0.9:5.1f}m", font=f_lab, fill=WHITE)
        imgs.append(im.convert("P", palette=Image.ADAPTIVE, colors=200))
    out = LAB / "nav_demo_walk.gif"
    imgs[0].save(out, save_all=True, append_images=imgs[1:],
                 duration=170, loop=0, optimize=True, disposal=2)
    print("wrote", out, out.stat().st_size // 1024, "KB", len(imgs), "frames")
    return out


if __name__ == "__main__":
    draw_path()
    draw_profile()
    draw_steps()
    draw_gif()
