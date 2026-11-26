"""M2-D07 导航寻路演示 —— 追踪镜头版（相机跟随圆柱体 agent）。

与 nav_demo_run.py 的区别：
- 原版是**固定俯视**相机（camera 段写死 0,132,92 -> 0,9,-2），整条路径一眼看全；
- 本脚本让相机**跟着 agent 走**：每帧按「agent 后方 + 上方」算出期望机位，做指数平滑后
  经 `script.eval` 调引擎脚本 API `camera(px,py,pz,tx,ty,tz)` 落到运行中的场景上，
  再 `capture.frame` 抓帧，得到第三人称追踪镜头序列。
- 因为相机逐帧变化，「世界 -> 屏幕」的投影矩阵也逐帧重算，红线叠加按帧投影
  （相机后方点按 w<=0 剔除，折线自动断开）。

做的事：
1. 起 r3e 宿主（1280x800，RPC 桥 8797），加载 assets/labs/nav_demo/nav_demo.r3e.json；
2. nav.bake 烘焙 + nav.stats 体检；
3. nav.nearest 吸起点/终点，nav.path 出路径，nav.smooth 出平滑路径；
4. 沿平滑路径等弧长取 CHASE_FRAMES 个点，逐帧：移动 agent -> 平滑机位 -> eval camera -> 抓帧；
5. 合成追踪镜头 GIF、关键帧拼图；最后把相机还原成场景俯视机位补一张总览图（画完整路径与起终点），
   数据落 evidence_chase/data_chase.json。

用法：
    python assets/labs/nav_demo/tools/nav_demo_chase.py

注意：本脚本**不加载** chase_cam.rhai。原因是该脚本每帧在 on_update 里硬设相机，
会覆盖 Python 侧算好的平滑机位；两者是两种用法（窗口实时看 vs 离线出片），不要同时开。
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
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(r"D:\rust3d-engine")
LAB = ROOT / "assets" / "labs" / "nav_demo"
EXE = ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe"
SCENE = LAB / "nav_demo.r3e.json"
EV = LAB / "evidence_chase"
PORT = 8797
TOKEN = "secret"
W, H = 1280, 800

CHASE_FRAMES = int(os.environ.get("CHASE_FRAMES", "48"))
AGENT_LIFT = 0.9          # agent 节点原点离地高度（圆柱高 1.8m，原点在中心）
CAM_BACK = 24.0           # 机位在 agent 后方的水平距离
CAM_HEIGHT = 16.0         # 机位相对 agent 的高度（比原俯视机位低得多，但仍高过 8m 的墙）
LOOK_AHEAD = 9.0          # 视线落点在 agent 前方的距离（让 agent 偏画面下方、前方路径可见）
LOOK_LIFT = 1.0           # 视线落点相对 agent 的抬高
CAM_FOV = 50.0

# 地形剖面（只依赖 x），与 nav_demo.r3e.json 的 terrain.heights 同源
KNOTS = [(-60.0, 0.0), (-35.0, 0.0), (-5.0, 7.0), (25.0, 19.0), (60.0, 19.0)]

RED = (255, 40, 40)
RED_DIM = (255, 120, 120)
WHITE = (255, 255, 255)


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


# ---------- 相机 / 投影 ----------
def cam_matrix(pos, target, fov_deg, w, h):
    """与引擎一致的相机公式：(look_at + directx 透视)，aspect = w/h。"""
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
    return np.array([
        [right[0] / (a * t), right[1] / (a * t), right[2] / (a * t), -np.dot(right, pos) / (a * t)],
        [tup[0] / t, tup[1] / t, tup[2] / t, -np.dot(tup, pos) / t],
        [f[0], f[1], f[2], -np.dot(f, pos)],
    ])


def project_vis(P, pts, w=W, h=H):
    """世界 -> 像素，同时返回可见性掩码（相机后方 w<=0 的点要丢掉，否则折线会翻转）。"""
    pts = np.atleast_2d(np.array(pts, dtype=float))
    hh = np.hstack([pts, np.ones((len(pts), 1))]) @ P.T
    wc = hh[:, 2]
    safe = np.where(np.abs(wc) < 1e-9, 1e-9, wc)
    ndc = hh[:, :2] / safe[:, None]
    scr = np.stack([(ndc[:, 0] + 1.0) * 0.5 * w, (1.0 - ndc[:, 1]) * 0.5 * h], axis=1)
    return scr, wc > 1e-6


def draw_polyline(d, pts, vis, fill, width):
    """按可见性分段画折线（相机后方的段直接断开）。"""
    seg = []
    for (x, y), ok in zip(pts, vis):
        if not ok:
            if len(seg) >= 2:
                d.line(seg, fill=fill, width=width, joint="curve")
            seg = []
            continue
        seg.append((float(x), float(y)))
    if len(seg) >= 2:
        d.line(seg, fill=fill, width=width, joint="curve")


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


def unit2(v):
    n = math.hypot(v[0], v[2])
    if n < 1e-9:
        return np.array([1.0, 0.0, 0.0])
    return np.array([v[0] / n, 0.0, v[2] / n])


def font(size, bold=False):
    """取中文字体：优先系统字体绝对路径，其次字体名，最后回退 PIL 默认（小字，仅兜底）。"""
    reg = Path(os.environ.get("WINDIR", r"C:\Windows")) / "Fonts"
    cands = []
    for nm in (("msyhbd.ttc" if bold else "msyh.ttc"), "simhei.ttf", "arialbd.ttf" if bold else "arial.ttf"):
        cands.append(str(reg / nm))
        cands.append(nm)
    for name in cands:
        try:
            return ImageFont.truetype(name, size)
        except OSError:
            continue
    return ImageFont.load_default()


def draw_chase_gif(data):
    """把追踪镜头过程帧合成 GIF：相机在动，红线按当前帧机位投影。"""
    frames = data["frames"]
    if len(frames) < 3:
        return None
    W2, H2 = 832, 520
    sx, sy = W2 / W, H2 / H
    f_lab = font(20, True)
    f_small = font(16)
    route = data["steps"]      # 48 个等弧长点：比 nav.smooth 的 8 点折线更贴合弯道
    imgs = []
    for idx, fr in enumerate(frames):
        k = fr["k"]
        im = Image.open(EV / fr["png"]).convert("RGB").resize((W2, H2))
        d = ImageDraw.Draw(im, "RGBA")
        P = np.array(fr["projection"], dtype=float).reshape(3, 4)
        # 未走过的整条路径：淡红线
        scr, vis = project_vis(P, route, W, H)
        draw_polyline(d, scr * np.array([sx, sy]), vis, RED_DIM + (150,), 3)
        # 已走过的部分：实红线
        walked = fr["walked"]
        if len(walked) >= 2:
            scr2, vis2 = project_vis(P, walked, W, H)
            draw_polyline(d, scr2 * np.array([sx, sy]), vis2, (20, 0, 0, 200), 7)
            draw_polyline(d, scr2 * np.array([sx, sy]), vis2, RED + (255,), 4)
        # agent 标记环
        a_scr, a_vis = project_vis(P, [fr["agent"]], W, H)
        if a_vis[0]:
            cx, cy = a_scr[0] * np.array([sx, sy])
            d.ellipse([cx - 10, cy - 10, cx + 10, cy + 10], outline=WHITE + (235,), width=3)
            d.ellipse([cx - 14, cy - 14, cx + 14, cy + 14], outline=RED + (200,), width=2)
        # 顶栏
        d.rectangle([0, 0, W2, 36], fill=(0, 0, 0, 150))
        prog = 100.0 * k / (len(frames) - 1)
        d.text((10, 8), f"追踪镜头 frame {k:02d}/{len(frames) - 1:02d}   进度 {prog:5.1f}%   "
                        f"地面高 {fr['agent'][1] - AGENT_LIFT:5.1f}m", font=f_lab, fill=WHITE)
        # 底栏：机位与目标距离
        d.rectangle([0, H2 - 30, W2, H2], fill=(0, 0, 0, 130))
        dist = math.dist(fr["cam_pos"], fr["agent"])
        d.text((10, H2 - 26), f"机位 ({fr['cam_pos'][0]:6.1f},{fr['cam_pos'][1]:5.1f},{fr['cam_pos'][2]:6.1f})"
                             f"   机位-目标 {dist:4.1f}m   fov {fr['fov']:.0f}°",
               font=f_small, fill=WHITE)
        imgs.append(im.convert("P", palette=Image.ADAPTIVE, colors=200))
    out = LAB / "nav_demo_chase.gif"
    imgs[0].save(out, save_all=True, append_images=imgs[1:], duration=110, loop=0,
                 optimize=True, disposal=2)
    print("wrote", out, out.stat().st_size // 1024, "KB", len(imgs), "frames")
    return out


def draw_contact_sheet(data):
    """关键帧拼图 4x2：一眼看完整段追踪过程。"""
    frames = data["frames"]
    pick = [frames[int(round(i * (len(frames) - 1) / 7))] for i in range(8)]
    W2, H2 = 624, 390
    sx, sy = W2 / W, H2 / H
    f_lab = font(18, True)
    sheet = Image.new("RGB", (W2 * 4, H2 * 2), (16, 16, 18))
    route = data["steps"]
    for i, fr in enumerate(pick):
        im = Image.open(EV / fr["png"]).convert("RGB").resize((W2, H2))
        d = ImageDraw.Draw(im, "RGBA")
        P = np.array(fr["projection"], dtype=float).reshape(3, 4)
        scr, vis = project_vis(P, route, W, H)
        draw_polyline(d, scr * np.array([sx, sy]), vis, RED_DIM + (150,), 2)
        if len(fr["walked"]) >= 2:
            scr2, vis2 = project_vis(P, fr["walked"], W, H)
            draw_polyline(d, scr2 * np.array([sx, sy]), vis2, (20, 0, 0, 200), 6)
            draw_polyline(d, scr2 * np.array([sx, sy]), vis2, RED + (255,), 3)
        a_scr, a_vis = project_vis(P, [fr["agent"]], W, H)
        if a_vis[0]:
            cx, cy = a_scr[0] * np.array([sx, sy])
            d.ellipse([cx - 9, cy - 9, cx + 9, cy + 9], outline=WHITE + (235,), width=2)
        d.rectangle([0, 0, W2, 28], fill=(0, 0, 0, 150))
        d.text((8, 5), f"k={fr['k']:02d}  {100.0 * fr['k'] / (len(frames) - 1):5.1f}%  "
                       f"h={fr['agent'][1] - AGENT_LIFT:5.1f}m", font=f_lab, fill=WHITE)
        sheet.paste(im, ((i % 4) * W2, (i // 4) * H2))
    out = LAB / "nav_demo_chase_contact.png"
    sheet.save(out)
    print("wrote", out, out.stat().st_size // 1024, "KB")
    return out


def draw_overview(rpc, data):
    """追踪镜头跑完后，切回场景俯视机位补一张总览对照图。

    追踪镜头只能看到身前几十米，整条路径大半落在相机后方（w<=0），没法在一张追踪帧里交代全貌，
    所以额外用场景自带的俯视机位抓一帧，投影矩阵按**这个**机位重算，画出完整路径与起终点。
    """
    cam_p, cam_t, fov = (0.0, 132.0, 92.0), (0.0, 9.0, -2.0), 46.0
    rpc.call("script.eval", source=f"fov({fov})")
    rpc.call("script.eval", source="camera({:.4f}, {:.4f}, {:.4f}, {:.4f}, {:.4f}, {:.4f})".format(*cam_p, *cam_t))
    time.sleep(0.35)
    png = EV / "overview.png"
    rpc.call("capture.frame", path=str(png))

    im = Image.open(png).convert("RGB")
    d = ImageDraw.Draw(im, "RGBA")
    P = cam_matrix(cam_p, cam_t, fov, W, H)
    f_lab = font(22, True)
    walked = data["frames"][-1]["walked"]
    scr, vis = project_vis(P, walked)
    draw_polyline(d, scr, vis, (20, 0, 0, 210), 9)
    draw_polyline(d, scr, vis, RED + (255,), 5)
    for label, pt, color in (("起点 start", data["start"], (60, 240, 120)),
                             ("终点 goal", data["goal"], (80, 140, 255))):
        s, v = project_vis(P, [pt])
        if v[0]:
            x, y = s[0]
            d.ellipse([x - 14, y - 14, x + 14, y + 14], outline=color + (255,), width=4)
            d.text((x + 18, y - 13), label, font=f_lab, fill=color)
    a_s, a_v = project_vis(P, [data["frames"][-1]["agent"]])
    if a_v[0]:
        ax, ay = a_s[0]
        d.ellipse([ax - 9, ay - 9, ax + 9, ay + 9], outline=WHITE + (255,), width=3)
    d.rectangle([0, 0, W, 40], fill=(0, 0, 0, 150))
    d.text((12, 9), f"nav_demo 追踪镜头跑完后俯视总览   路径 {data['path_length_2d']:.1f}m   "
                    f"smooth {data['smooth_length']:.1f}m   bake 可走格 {data['bake']['walkable']} "
                    f"({data['bake']['walkableRatio'] * 100:.1f}%)", font=f_lab, fill=WHITE)
    out = LAB / "nav_demo_chase_overview.png"
    im.save(out)
    print("wrote", out, out.stat().st_size // 1024, "KB")
    data["overview"] = {"png": str(png.relative_to(EV)), "camera": {"pos": list(cam_p), "target": list(cam_t),
                                                                   "fov": fov},
                        "projection": P.flatten().tolist(), "out": str(out.relative_to(LAB))}
    return out


def main():
    EV.mkdir(parents=True, exist_ok=True)
    (EV / "frames").mkdir(parents=True, exist_ok=True)
    log = EV / "host.log"
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
        print("scene:", Path(info["scene"]).name, "nodes:", info["nodes"],
              "script:", Path(str(info.get("script", ""))).name, flush=True)

        # #start_marker / #goal_marker 是 2×2×2 的实心方块，会把 1.8m 高的 agent 整个包住，
        # 追踪镜头第一帧就看不见圆柱体。这两块只是画面标记（# 前缀不参与烘焙），这里直接摘掉，
        # 起终点改由出图时绘制的圈 + 文字标注。
        for m in ("#start_marker", "#goal_marker"):
            try:
                rpc.call("scene.remove", node=m)
            except Exception as e:
                print("remove", m, "skip:", e, flush=True)

        bake = rpc.call("nav.bake")
        stats = rpc.call("nav.stats")
        print("bake:", json.dumps(bake, ensure_ascii=False), flush=True)

        # ---- 路径 ----
        start_q = [-50.0, ground(-50.0), -12.0]
        goal_q = [48.0, ground(48.0), -12.0]
        start = rpc.call("nav.nearest", point=start_q, radius=8.0)["point"]
        goal = rpc.call("nav.nearest", point=goal_q, radius=8.0)["point"]
        path = rpc.call("nav.path", **{"from": start, "to": goal})
        raw_pts = path.get("points") or []
        if not path.get("found") or len(raw_pts) < 2:
            print("寻路未成功，终止")
            return 1
        sm = rpc.call("nav.smooth", points=raw_pts)
        smooth_pts = sm.get("points") or raw_pts
        print("path", path.get("count"), "pts -> smooth", sm.get("count"),
              "len", sm.get("length"), flush=True)

        steps, total_len = arc_sample(smooth_pts, CHASE_FRAMES)

        # ---- 追踪镜头：逐帧移动 agent + 平滑机位 + 抓帧 ----
        rpc.call("script.eval", source=f"fov({CAM_FOV})")
        dir_s = unit2(np.array(steps[1]) - np.array(steps[0])) if len(steps) > 1 else np.array([1.0, 0.0, 0.0])
        cam_pos = np.array(steps[0], dtype=float)
        cam_tgt = np.array(steps[0], dtype=float)

        frames = []
        walked = []
        for k, p in enumerate(steps):
            agent = [p[0], ground(p[0]) + AGENT_LIFT, p[2]]
            walked.append(agent)
            rpc.call("scene.settransform", node="agent", position=agent)

            # 前进方向（取下一段，末帧沿用上一段），做方向平滑避免转向抖动
            nxt = np.array(steps[min(k + 1, len(steps) - 1)])
            prv = np.array(steps[max(k - 1, 0)])
            d_raw = unit2(nxt - prv) if k not in (0, len(steps) - 1) else unit2(nxt - np.array(p))
            dir_s = dir_s * (1.0 - 0.35) + d_raw * 0.35
            dir_s = unit2(dir_s)

            a_p = np.array(agent)
            # 机位按当前 agent 位置 + 平滑后的前进方向解析给出。steps 已是等弧长平滑采样，
            # 机位序列本身连续；这里不再做指数平滑，否则相机会永远滞后 agent 若干米、构图乱飘。
            cam_pos = a_p + np.array([-dir_s[0] * CAM_BACK, CAM_HEIGHT, -dir_s[2] * CAM_BACK])
            cam_tgt = a_p + np.array([dir_s[0] * LOOK_AHEAD, LOOK_LIFT, dir_s[2] * LOOK_AHEAD])

            rpc.call("script.eval",
                     source="camera({:.4f}, {:.4f}, {:.4f}, {:.4f}, {:.4f}, {:.4f})".format(
                         cam_pos[0], cam_pos[1], cam_pos[2], cam_tgt[0], cam_tgt[1], cam_tgt[2]))
            time.sleep(0.12)   # 给主循环一帧把相机写进渲染状态，避免抓到上一帧机位
            fp = EV / "frames" / f"chase_{k:03d}.png"
            rpc.call("capture.frame", path=str(fp))

            P = cam_matrix(cam_pos, cam_tgt, CAM_FOV, W, H)
            frames.append({
                "k": k,
                "agent": [round(float(v), 3) for v in agent],
                "cam_pos": [round(float(v), 3) for v in cam_pos],
                "cam_target": [round(float(v), 3) for v in cam_tgt],
                "fov": CAM_FOV,
                "projection": P.flatten().tolist(),
                "walked": [[round(float(c), 3) for c in w] for w in walked],
                "png": f"frames/chase_{k:03d}.png",
            })
            if k % 8 == 0 or k == len(steps) - 1:
                print(f"  frame {k:02d}: agent ({agent[0]:6.1f},{agent[1]:5.1f},{agent[2]:6.1f})  "
                      f"cam ({cam_pos[0]:6.1f},{cam_pos[1]:5.1f},{cam_pos[2]:6.1f})", flush=True)

        data = {
            "scene": str(SCENE),
            "render": {"width": W, "height": H},
            "camera_rig": {"back": CAM_BACK, "height": CAM_HEIGHT, "look_ahead": LOOK_AHEAD,
                           "look_lift": LOOK_LIFT, "fov": CAM_FOV},
            "bake": bake,
            "stats": stats,
            "start": start,
            "goal": goal,
            "smooth_points": smooth_pts,
            "smooth_length": sm.get("length"),
            "path_length_2d": total_len,
            "steps": steps,
            "frames": frames,
            "terrain_profile": KNOTS,
        }
        # 追踪机位还原成俯视机位补一张总览（宿主还活着时做，抓完再落盘 data）
        draw_overview(rpc, data)

        out = EV / "data_chase.json"
        out.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
        print("wrote", out, flush=True)

        draw_chase_gif(data)
        draw_contact_sheet(data)
        return 0
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=10)
        except Exception:
            proc.kill()


if __name__ == "__main__":
    sys.exit(main())
