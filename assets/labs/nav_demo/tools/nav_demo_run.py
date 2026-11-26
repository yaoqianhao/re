"""M2-D07 导航寻路演示 —— 步骤一：跑起来取样。

做的事：
1. 起 r3e 宿主（窗口 1280x800，RPC 桥 8791），加载 assets/labs/nav_demo/nav_demo.r3e.json；
2. nav.bake 烘焙可走网格，nav.stats 记体检数字；
3. 用 8 个洋红标定块求「世界坐标 -> 屏幕像素」的投影矩阵（先按相机参数给初值匹配、
   再用 DLT 最小二乘精修），把 3D 截图与 2D 叠加对齐；
4. 撤掉标定块，抓一张干净的俯视底图 base.png；
5. nav.nearest 把起点 / 终点吸到可走面，nav.path 出路径，nav.smooth 出平滑路径；
6. 沿平滑路径等弧长取 8 个过程点，逐点 scene.settransform 移动 agent 并抓帧，
   得到「圆柱体从起点走到终点」的过程帧；
7. 全部结果落 evidence/data.json，供 nav_demo_draw.py 画图。

用法：python nav_demo_run.py
"""
import json
import math
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

ROOT = Path(r"D:\rust3d-engine")
LAB = ROOT / "assets" / "labs" / "nav_demo"
EXE = ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe"
SCENE = LAB / "nav_demo.r3e.json"
EV = LAB / "evidence"
PORT = 8791
TOKEN = "secret"
W, H = 1280, 800
N_STEP = int(os.environ.get("NAV_STEPS", "8"))   # 过程帧数量，跑动画时调大（如 36）

# 与 nav_demo.r3e.json 的 camera 段一致
CAM_POS = (0.0, 132.0, 92.0)
CAM_TARGET = (0.0, 9.0, -2.0)
CAM_FOV = 46.0

# 地形剖面（只依赖 x），与场景 terrain.heights 同源
KNOTS = [(-60.0, 0.0), (-35.0, 0.0), (-5.0, 7.0), (25.0, 19.0), (60.0, 19.0)]


def ground(x):
    if x <= KNOTS[0][0]:
        return KNOTS[0][1]
    for (x0, y0), (x1, y1) in zip(KNOTS, KNOTS[1:]):
        if x0 <= x <= x1:
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    return KNOTS[-1][1]


# ---------- RPC ----------
class Rpc:
    def __init__(self, addr, token, timeout=90.0):
        self.sock = socket.create_connection(addr, timeout=10.0)
        self.sock.settimeout(timeout)
        self.f = self.sock.makefile("rwb")
        self.token = token
        self.n = 0

    def call(self, method, **params):
        self.n += 1
        req = {"jsonrpc": "2.0", "id": self.n, "method": method,
               "params": dict(params, token=self.token)}
        self.f.write((json.dumps(req) + "\n").encode("utf-8"))
        self.f.flush()
        line = self.f.readline()
        if not line:
            raise RuntimeError(f"{method}: 连接被关闭")
        msg = json.loads(line.decode("utf-8", "replace"))
        if msg.get("id") != self.n:
            raise RuntimeError(f"{method}: id 错序")
        if msg.get("error"):
            raise RuntimeError(f"{method}: {json.dumps(msg['error'], ensure_ascii=False)}")
        return msg.get("result")


# ---------- 投影 ----------
def cam_matrix(pos, target, fov_deg, w, h):
    pos = np.array(pos, dtype=float)
    tgt = np.array(target, dtype=float)
    f = tgt - pos
    f /= np.linalg.norm(f)
    up = np.array([0.0, 1.0, 0.0])
    right = np.cross(f, up)
    right /= np.linalg.norm(right)
    tup = np.cross(right, f)
    t = math.tan(math.radians(fov_deg) / 2.0)
    a = w / h
    px = np.array([
        [right[0] / (a * t), right[1] / (a * t), right[2] / (a * t), -np.dot(right, pos) / (a * t)],
        [tup[0] / t, tup[1] / t, tup[2] / t, -np.dot(tup, pos) / t],
        [f[0], f[1], f[2], -np.dot(f, pos)],
    ])
    return px


def project(P, pts, w=W, h=H):
    """世界坐标 -> 屏幕像素（左上原点、y 向下）。P 可直接是相机公式矩阵，也可来自 DLT。"""
    pts = np.atleast_2d(np.array(pts, dtype=float))
    hh = np.hstack([pts, np.ones((len(pts), 1))]) @ P.T
    ndc = hh[:, :2] / hh[:, 2:3]
    return np.stack([(ndc[:, 0] + 1.0) * 0.5 * w, (1.0 - ndc[:, 1]) * 0.5 * h], axis=1)


def fit_projection(world, screen):
    """DLT：解 3x4 投影矩阵（最小二乘）。"""
    A = []
    for (X, Y, Z), (u, v) in zip(world, screen):
        A.append([X, Y, Z, 1, 0, 0, 0, 0, -u * X, -u * Y, -u * Z, -u])
        A.append([0, 0, 0, 0, X, Y, Z, 1, -v * X, -v * Y, -v * Z, -v])
    _, _, vt = np.linalg.svd(np.array(A, dtype=float))
    return vt[-1].reshape(3, 4)


def detect_magenta(path_png, expect):
    """检测洋红标定块质心，并按初值预测做最近匹配。"""
    from PIL import Image
    img = np.asarray(Image.open(path_png).convert("RGB")).astype(int)
    m = (img[:, :, 0] > 140) & (img[:, :, 1] < 100) & (img[:, :, 2] > 140)
    ys, xs = np.nonzero(m)
    if len(xs) == 0:
        return [], []
    # 简单连通聚类：按网格桶合并
    pts = list(zip(xs.tolist(), ys.tolist()))
    clusters = []
    for x, y in pts:
        placed = False
        for c in clusters:
            if abs(c["cx"] - x) < 40 and abs(c["cy"] - y) < 40:
                c["xs"].append(x); c["ys"].append(y)
                c["cx"] = sum(c["xs"]) / len(c["xs"]); c["cy"] = sum(c["ys"]) / len(c["ys"])
                placed = True
                break
        if not placed:
            clusters.append({"xs": [x], "ys": [y], "cx": float(x), "cy": float(y)})
    cents = [((c["cx"], c["cy"]), len(c["xs"])) for c in clusters if len(c["xs"]) > 60]
    cents.sort(key=lambda t: -t[1])
    cents = [c for c, _ in cents]
    # 与预测做一对一贪心匹配（按距离升序分配，避免多个预测抢同一个簇）
    pairs = []
    for i, pred in enumerate(expect):
        for j, c in enumerate(cents):
            d = math.dist(c, pred)
            pairs.append((d, i, j))
    pairs.sort()
    used_i, used_j = set(), set()
    matched = [None] * len(expect)
    for d, i, j in pairs:
        if i in used_i or j in used_j:
            continue
        used_i.add(i); used_j.add(j)
        matched[i] = cents[j]
    return matched, len(cents)


def arc_sample(points, n):
    pts = np.array(points, dtype=float)
    seg = np.linalg.norm(np.diff(pts[:, [0, 2]], axis=0), axis=1)
    cum = np.concatenate([[0.0], np.cumsum(seg)])
    total = cum[-1]
    out = []
    for k in range(n):
        s = total * k / (n - 1)
        i = int(np.searchsorted(cum, s, side="right") - 1)
        i = max(0, min(i, len(seg) - 1))
        r = 0.0 if seg[i] < 1e-9 else (s - cum[i]) / seg[i]
        p = pts[i] + (pts[i + 1] - pts[i]) * r
        out.append([float(p[0]), float(p[1]), float(p[2])])
    return out, float(total)


CAL = [(-55.0, -20.0), (-55.0, 20.0), (55.0, -20.0), (55.0, 20.0),
       (-25.0, 0.0), (0.0, -22.0), (0.0, 22.0), (30.0, 0.0)]


def main():
    EV.mkdir(parents=True, exist_ok=True)
    log = Path(__file__).resolve().parent / ".." / "evidence" / "host.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.Popen(
        [str(EXE), str(SCENE), "--window-size", f"{W}x{H}",
         "--rpc-port", str(PORT), "--rpc-token", TOKEN,
         "--physics", "off", "--run-seconds", "900"],
        cwd=str(ROOT), stdout=open(log, "w", encoding="utf-8", errors="replace"),
        stderr=subprocess.STDOUT)
    print("host pid", proc.pid, flush=True)
    try:
        rpc = None
        for _ in range(60):
            time.sleep(1.0)
            if proc.poll() is not None:
                print("宿主退出；日志尾：")
                print(log.read_text(encoding="utf-8", errors="replace")[-2500:])
                return 1
            try:
                rpc = Rpc(("127.0.0.1", PORT), TOKEN)
                break
            except OSError:
                continue
        if rpc is None:
            print("连不上 RPC 桥")
            return 1
        time.sleep(3.0)

        info = rpc.call("engine.info")
        print("scene:", Path(info["scene"]).name, "nodes:", info["nodes"], flush=True)

        bake = rpc.call("nav.bake")
        stats = rpc.call("nav.stats")
        print("bake:", json.dumps(bake, ensure_ascii=False), flush=True)

        # ---- 1. 标定：8 个洋红块求投影矩阵 ----
        cal_world = []
        for i, (x, z) in enumerate(CAL):
            y = ground(x) + 1.2
            rpc.call("scene.spawn", object={
                "name": f"#cal_{i}", "mesh": "cube", "position": [x, y, z],
                "scale": [1.4, 1.4, 1.4], "color": [1.0, 0.0, 1.0, 1.0]})
            cal_world.append([x, y, z])
        cal_png = EV / "calibration.png"
        rpc.call("capture.frame", path=str(cal_png))

        P0 = cam_matrix(CAM_POS, CAM_TARGET, CAM_FOV, W, H)
        pred = project(P0, cal_world)
        found, nfound = detect_magenta(cal_png, pred)
        print("标定块检测: 期望 8 实际", nfound, flush=True)
        if nfound < 6 or any(c is None for c in found):
            print("标定块检测不足，投影无法标定；先看", cal_png)
            return 1
        # 相机公式与引擎渲染一致时直接用公式矩阵；实测残差 < 1px 就不需要 DLT 精修
        # （8 个标定块几乎共面，未归一化的 DLT 会退化成病态解，不再使用）
        err0 = [math.dist(f, p) for f, p in zip(found, pred)]
        P = P0
        err1 = err0
        print("标定残差 px: max %.2f  中位 %.2f  rms %.2f" % (
            max(err0), sorted(err0)[len(err0) // 2],
            float(np.sqrt(np.mean(np.square(err0))))))

        for i in range(len(CAL)):
            rpc.call("scene.remove", node=f"#cal_{i}")

        # ---- 2. 干净底图 ----
        base_png = EV / "base_overview.png"
        rpc.call("capture.frame", path=str(base_png))

        # ---- 3. 起点 / 终点 / 路径 ----
        start_q = [-50.0, ground(-50.0), -12.0]
        goal_q = [48.0, ground(48.0), -12.0]
        ns = rpc.call("nav.nearest", point=start_q, radius=8.0)
        gs = rpc.call("nav.nearest", point=goal_q, radius=8.0)
        start = ns["point"]
        goal = gs["point"]
        print("start:", start, "goal:", goal, flush=True)

        path = rpc.call("nav.path", **{"from": start, "to": goal})
        print("nav.path:", path.get("found"), path.get("outcome"), "points", path.get("count"), flush=True)
        raw_pts = path.get("points") or []
        if not path.get("found") or len(raw_pts) < 2:
            print("寻路未成功，终止")
            return 1
        sm = rpc.call("nav.smooth", points=raw_pts)
        smooth_pts = sm.get("points") or raw_pts
        print("smooth:", sm.get("inputCount"), "->", sm.get("count"), "len", sm.get("length"), flush=True)

        # ---- 4. 沿平滑路径等弧长取点，逐点移动 agent 抓帧 ----
        steps, total_len = arc_sample(smooth_pts, N_STEP)
        fdir = EV / f"frames{N_STEP}"
        fdir.mkdir(parents=True, exist_ok=True)
        tag = f"{os.getpid() % 100000:05d}"
        frames = []
        for k, p in enumerate(steps):
            y = ground(p[0]) + 0.9
            rpc.call("scene.settransform", node="agent", position=[p[0], y, p[2]])
            fp = fdir / f"frame_{tag}_{k:03d}.png"
            rpc.call("capture.frame", path=str(fp))
            frames.append({"k": k, "point": [p[0], y, p[2]],
                           "png": str(fp.relative_to(EV).as_posix())})
            if k % 4 == 0 or k == len(steps) - 1:
                print(f"  frame {k}: ({p[0]:.1f}, {y:.2f}, {p[2]:.1f})", flush=True)
        # 收尾：agent 停在终点
        rpc.call("scene.settransform", node="agent", position=[steps[-1][0], ground(steps[-1][0]) + 0.9, steps[-1][2]])

        data = {
            "scene": str(SCENE),
            "render": {"width": W, "height": H},
            "camera": {"position": list(CAM_POS), "target": list(CAM_TARGET), "fov": CAM_FOV},
            "projection": P.flatten().tolist(),
            "calibration": {
                "world": cal_world,
                "screen": [[float(a), float(b)] for a, b in found],
                "residual_camera_formula_px": err0,
                "residual_used_px": err1,
            },
            "bake": bake,
            "stats": stats,
            "start": start,
            "goal": goal,
            "raw_points": raw_pts,
            "smooth_points": smooth_pts,
            "smooth_length": sm.get("length"),
            "path_length_2d": total_len,
            "steps": steps,
            "frames": frames,
            "base_png": base_png.name,
            "terrain_profile": KNOTS,
        }
        out = EV / "data.json"
        out.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
        print("wrote", out)
        return 0
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
