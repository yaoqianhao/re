"""重拍 z=-50 横缝证据图（H-01B 修复后构建）。

每个机位独立起一个进程：脚本 on_start 里把相机钉死、盖上角标，跑到 `--capture-after`
帧后由 `--capture-frame` 离屏读回一张 PNG 再退出（不抢前台、不受窗口遮挡影响，
分辨率与 `--window-size` 一致）。逐张记录：启动参数、相机位姿、fov、两侧 LOD
（按 crates/r3e-terrain/src/lod.rs 的 64/200 阈值与块中心距离推算）、MD5、拍摄时间。

探针机位（probe_*）额外把清屏底色改成洋红 `set_background(1,0,1)`：任何「从缝里透出
背景」的像素都会是洋红而不是地形色，配合 scan_seam_holes.py 的连通域判定可以机械地
证伪/证实「坡面穿透」。

用法：
    python capture_seam_evidence.py --dry-run
    python capture_seam_evidence.py
    python capture_seam_evidence.py --only probe
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

LAB = Path(__file__).resolve().parents[1]
ROOT = LAB.parents[2]
EVID = LAB / "evidence"
TEMP = Path(__file__).resolve().parent
EXE = ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe"
SCENE = LAB / "terrain_roam.r3e.json"

BLOCK = 50.0
ORIGIN = (-100.0, -100.0)          # patch origin
NEAR, FAR = 64.0, 200.0            # crates/r3e-terrain/src/lod.rs LOD_DISTANCES
GROUND_KNOTS = [(-100.0, 0.0), (-60.0, 0.0), (-40.0, 5.358), (-20.0, 29.193), (100.0, 29.193)]


def ground(x: float) -> float:
    for (x0, y0), (x1, y1) in zip(GROUND_KNOTS, GROUND_KNOTS[1:]):
        if x0 <= x <= x1:
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    return GROUND_KNOTS[-1][1]


def lod_of(bx: int, bz: int, eye) -> str:
    cx, cz = ORIGIN[0] + (bx + 0.5) * BLOCK, ORIGIN[1] + (bz + 0.5) * BLOCK
    d = math.hypot(cx - eye[0], cz - eye[2])
    return "L0" if d <= NEAR else ("L1" if d <= FAR else "L2")


def lod_line(eye, seam_x: float, seam_z: float = -50.0) -> str:
    """接缝两侧各块的档位（块中心到相机 XZ 距离，与 terrain_scene::plan 同口径）。"""
    bx = int((seam_x - ORIGIN[0]) // BLOCK)
    bz_near = int((seam_z - 0.001 - ORIGIN[1]) // BLOCK)     # 接缝南侧那一行
    a = lod_of(bx, bz_near, eye)
    b = lod_of(bx, bz_near + 1, eye)
    d0 = math.hypot(ORIGIN[0] + (bx + 0.5) * BLOCK - eye[0], ORIGIN[1] + (bz_near + 0.5) * BLOCK - eye[2])
    d1 = math.hypot(ORIGIN[0] + (bx + 0.5) * BLOCK - eye[0], ORIGIN[1] + (bz_near + 1.5) * BLOCK - eye[2])
    return f"({bx},{bz_near})={a}@d{d0:.1f}m / ({bx},{bz_near + 1})={b}@d{d1:.1f}m"


def st(name, memo, eye, target, fov, seam_x, bg=None):
    return {"name": name, "memo": memo, "eye": eye, "target": target,
            "fov": fov, "seam_x": seam_x, "bg": bg}


G25, G50 = ground(-25.0), ground(-50.0)

STATIONS = [
    st("seam_00_x-25_close_L0L0", "坡面近距正对 z=-50（x=-25，历史错位 7.30m 处），两侧 L0",
       (-25.0, G25 + 4.0, -68.0), (-25.0, G25 - 0.5, -50.0), 30.0, -25.0),
    st("seam_01_x-25_mid_L0L0", "坡面中距正对 z=-50（x=-25），两侧 L0",
       (-25.0, G25 + 6.0, -85.0), (-25.0, G25 - 1.0, -50.0), 40.0, -25.0),
    st("seam_02_x-50_corner_L0L1", "四块交汇角（x=-50, z=-50）：南侧 L0 / 北侧 L1",
       (-50.0, G50 + 5.0, -90.0), (-50.0, G50 + 0.5, -50.0), 40.0, -50.0),
    st("seam_03_x-25_cross_L0L1", "中距正对 z=-50（x=-25）：南侧 L0 / 北侧 L1",
       (-25.0, G25 + 8.0, -112.0), (-25.0, G25 - 1.0, -50.0), 40.0, -25.0),
    st("seam_04_x-25_L1L1", "远距正对 z=-50（x=-25）：两侧 L1",
       (-25.0, G25 + 14.0, -175.0), (-25.0, G25, -50.0), 32.0, -25.0),
    st("seam_05_x-25_L2L2", "超远距正对 z=-50（x=-25）：两侧 L2",
       (-25.0, G25 + 20.0, -280.0), (-25.0, G25, -50.0), 22.0, -25.0),
    st("seam_06_x-25_cross_L2L1_north", "自北向南正对 z=-50（x=-25）：北侧 L1 / 南侧 L2（历史失败组合的镜像）",
       (-25.0, G25 + 16.0, 130.0), (-25.0, G25 - 2.0, -50.0), 25.0, -25.0),
    st("seam_07_plateau_x25_control_L2L1", "台地负对照（x=25 平坦段）：南侧 L2 / 北侧 L1，本应只剩着色差",
       (25.0, 29.193 + 16.0, 130.0), (25.0, 29.193, -50.0), 28.0, 25.0),
    st("seam_08_grazing_x-25", "掠射视角：相机在 50° 坡面上方 1.6m，正对 z=-50（缝被放大到近景）",
       (-25.0, G25 + 1.6, -60.0), (-25.0, G25 + 0.6, -50.0), 65.0, -25.0),
    st("seam_09_grazing_x-50_corner", "掠射视角：相机在 15° 坡面上方 1.4m，正对四块交汇角",
       (-50.0, G50 + 1.4, -60.0), (-50.0, G50 + 0.6, -50.0), 65.0, -50.0),
    st("seam_10_overview_200m", "整张 200×200 收尾俯视：z=-50 接缝线横贯全场",
       (-50.0, 220.0, -150.0), (0.0, 0.0, 10.0), 60.0, -25.0),
    st("probe_00_magenta_x-25_cross_L0L1", "透光探针：同 seam_03 机位，清屏底色改洋红（1,0,1），缝里若透背景必为洋红",
       (-25.0, G25 + 8.0, -112.0), (-25.0, G25 - 1.0, -50.0), 40.0, -25.0, bg=(1.0, 0.0, 1.0)),
    st("probe_01_magenta_x-50_corner", "透光探针：同 seam_02 机位，清屏底色改洋红（1,0,1）",
       (-50.0, G50 + 5.0, -90.0), (-50.0, G50 + 0.5, -50.0), 40.0, -50.0, bg=(1.0, 0.0, 1.0)),
]


def esc(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def script_for(s: dict) -> str:
    px, py, pz = s["eye"]
    tx, ty, tz = s["target"]
    lines = [
        "// 自动生成：" + s["name"] + "。相机在 on_start 钉死，本进程只拍这一张。",
        "fn on_start() {",
        f"    camera({px:.3f}, {py:.3f}, {pz:.3f}, {tx:.3f}, {ty:.3f}, {tz:.3f});",
        f"    fov({s['fov']:.1f});",
    ]
    if s["bg"] is not None:
        r, g, b = s["bg"]
        lines.append(f"    set_background({r:.2f}, {g:.2f}, {b:.2f});")
    hud = (f"{s['name']}  eye=({px:.1f},{py:.1f},{pz:.1f})  "
           f"target=({tx:.1f},{ty:.1f},{tz:.1f})  fov={s['fov']:.0f}  seam z=-50")
    lines += [
        f'    text_add("hud", "{esc(hud)}", 14.0, 12.0, 16.0, 1.0, 0.85, 0.25);',
        '    log("seam-cam ' + s["name"] + f' eye=({px:.3f},{py:.3f},{pz:.3f}) target=({tx:.3f},{ty:.3f},{tz:.3f}) fov={s["fov"]:.1f}");',
        "}",
        "",
    ]
    return "\n".join(lines)


def md5_of(path: Path) -> str:
    h = hashlib.md5()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--only", default="", help="只跑名字包含该子串的机位")
    ap.add_argument("--frames", type=int, default=90)
    ap.add_argument("--size", default="1280x720")
    args = ap.parse_args()

    picked = [s for s in STATIONS if args.only in s["name"]]
    if args.dry_run:
        for s in picked:
            print(f"{s['name']}\n  eye={s['eye']} target={s['target']} fov={s['fov']} bg={s['bg']}\n"
                  f"  LOD {lod_line(s['eye'], s['seam_x'])}\n  {s['memo']}")
        return 0

    if not EXE.exists():
        print(f"缺少 {EXE}，先 cargo build", flush=True)
        return 2
    EVID.mkdir(exist_ok=True)

    rows, failures = [], 0
    for s in picked:
        sc = TEMP / f"{s['name']}.rhai"
        sc.write_text(script_for(s), encoding="utf-8")
        out = EVID / f"{s['name']}.png"
        if out.exists():
            out.unlink()
        cli = [str(EXE), str(SCENE), str(sc),
               "--physics", "on", "--physics-backend", "rapier",
               "--window-size", args.size,
               "--capture-frame", str(out), "--capture-after", str(args.frames)]
        started = time.time()
        proc = subprocess.run(cli, cwd=str(ROOT), capture_output=True, text=True,
                              encoding="utf-8", errors="replace", timeout=300)
        took = time.time() - started
        (TEMP / f"{s['name']}.log").write_text((proc.stdout or "") + (proc.stderr or ""),
                                               encoding="utf-8", errors="replace")
        ok = proc.returncode == 0 and out.exists() and out.stat().st_size > 0
        failures += 0 if ok else 1
        rows.append({
            "name": s["name"], "memo": s["memo"],
            "eye": s["eye"], "target": s["target"], "fov": s["fov"],
            "background_override": s["bg"],
            "seam_x": s["seam_x"], "lod": lod_line(s["eye"], s["seam_x"]),
            "cli": " ".join(cli),
            "returncode": proc.returncode,
            "elapsed_s": round(took, 1),
            "shot_at": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
            "file": out.name if ok else "",
            "bytes": out.stat().st_size if out.exists() else 0,
            "md5": md5_of(out) if ok else "",
        })
        print(("[OK]   " if ok else "[FAIL] ") +
              f"{s['name']}  {took:.1f}s  {rows[-1]['bytes']} bytes  {rows[-1]['md5']}", flush=True)
        if not ok:
            tail = ((proc.stdout or "") + (proc.stderr or "")).strip().splitlines()
            print("       " + (tail[-1][:200] if tail else "无输出"), flush=True)

    manifest = EVID / "seam_manifest.json"
    manifest.write_text(json.dumps({
        "generated_at": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
        "exe": str(EXE), "scene": str(SCENE), "size": args.size, "frames": args.frames,
        "lod_thresholds": {"L0": f"<= {NEAR}m", "L1": f"<= {FAR}m", "L2": f"> {FAR}m"},
        "station_count": len(rows), "failures": failures, "stations": rows,
    }, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"\n{len(rows)} 个机位，失败 {failures}；清单 {manifest}", flush=True)
    return 0 if failures == 0 else 3


if __name__ == "__main__":
    sys.exit(main())
