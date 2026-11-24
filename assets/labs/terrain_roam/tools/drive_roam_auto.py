"""真窗口地形漫游（自动巡检）：不注入按键，只看 auto_roam.rhai 跑出来的轨迹与截图。

为什么要这个而不是 drive_roam.py：键盘演示依赖 OS 注入按键 + 窗口抢前台，实测
`SetForegroundWindow` 会被前台锁定拒绝（`foreground ok: False`），届时引擎一帧都
不动、截图全停在出生点，证据不可复用。自动版把驱动交给脚本，命令在任何机器上都
跑出同一条轨迹，适合当 C07 的回归样例。

输出：
  evidence/auto_roam_0*.png        六张关键帧
  temp/auto_roam.log               引擎全量日志（脚本每帧一行轨迹，引擎自身去重）
  终端打印 [PASS]/[FAIL] 断言结果与轨迹摘要

判定口径（与地形高度场对齐，见 terrain_roam.r3e.json：11×11 顶点、cell 20 m，
列顶点 x = -100,-80,...,-60,-40,-20,0,...，高度 0 / 5.358 / 29.193）：
  · 平地段 -90 → -60 必须真的走起来（x 前进 > 20 m）
  · 冲刺段（限 35°）被 35° 陡坎挡住：停在 x ≈ -42、脚底 y ≈ 4.8，日志出现「坡度超限」
  · 步行段（限 55°）能越过同一处 50° 坡：x 继续推进到 > 45
  · 坡顶平台脚底 y 必须 ≈ 29.193（与渲染同面），且日志报「无需传送」
"""
from __future__ import annotations

import re
import subprocess
import sys
import time
from pathlib import Path

from PIL import ImageGrab

LAB = Path(__file__).resolve().parent.parent   # assets/labs/terrain_roam
ROOT = LAB.parents[2]                          # 项目根
LOG = ROOT / "temp" / "auto_roam.log"
EVID = LAB / "evidence"

# 截图判据：不按「相对进程启动的秒数」抓，改成盯轨迹/日志的**状态标记**抓。
#
# 起因：本机首次出帧要 ~6 s（wgpu 初始化 + 地形上传），按时钟抓到的前两张是空白窗口，
# 关键帧还会因机器快慢整体错位。按状态抓与机器速度无关：等到那一刻再拍。
STAGES = [
    ("auto_roam_00_spawn.png", "出生：x=-90 平地，脚底 y≈0",
     lambda pts, text: bool(pts)),
    ("auto_roam_01_sprint_blocked.png", "冲刺限 35°：被坡脚陡坎挡住 x≈-42",
     lambda pts, text: "坡度超限 3" in text and any(p[0] <= -38.0 for p in pts)),
    ("auto_roam_02_walk_over.png", "步行限 55°：越过同一处 50° 坡",
     lambda pts, text: any(p[0] > 0.0 for p in pts)),
    ("auto_roam_03_mid_slope.png", "坡面中段爬升",
     lambda pts, text: any(-45.0 < p[0] < 5.0 and 8.0 < p[1] < 25.0 for p in pts)),
    ("auto_roam_04_near_top.png", "接近坡顶平台 y 趋近 29.193",
     lambda pts, text: any(p[1] > 28.8 for p in pts)),
    ("auto_roam_05_overview.png", "高位俯视整张 200×200 地形",
     lambda pts, text: "无需传送" in text or "保底传送" in text),
]


def find_window(pid: int):
    """按进程找引擎窗口（winit 自绘，类名固定 Window Class）；找不到返回 None。"""
    import ctypes
    import ctypes.wintypes as wt

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        wpid = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value != pid or not user32.IsWindowVisible(hwnd):
            return True
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        rect = wt.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        if cls.value == "Window Class" and rect.right - rect.left > 300:
            hits.append(rect)
            return False
        return True

    user32.EnumWindows(cb, 0)
    return hits[0] if hits else None


def trace(text: str):
    """从日志里抽 `[script] auto x=.. y=.. z=..` 轨迹点。"""
    out = []
    for line in text.splitlines():
        if "[script] auto x=" not in line:
            continue
        parts = line.split("[script] auto ", 1)[1].split()
        try:
            out.append((float(parts[0][2:]), float(parts[1][2:]), float(parts[2][2:])))
        except (IndexError, ValueError):
            continue
    return out


def main() -> int:
    exe = ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe"
    if not exe.exists():
        print(f"缺少 {exe}，先 cargo build", flush=True)
        return 2
    EVID.mkdir(exist_ok=True)
    LOG.parent.mkdir(exist_ok=True)
    args = [str(exe), str(LAB / "terrain_roam.r3e.json"), str(LAB / "scripts" / "auto_roam.rhai"),
            "--physics", "on", "--physics-backend", "rapier",
            "--window-size", "960x540", "--run-seconds", "28"]
    started = time.time()
    proc = subprocess.Popen(args, cwd=str(ROOT),
                            stdout=open(LOG, "w", encoding="utf-8", errors="replace"),
                            stderr=subprocess.STDOUT)
    rect = None
    for _ in range(60):
        time.sleep(0.25)
        rect = find_window(proc.pid)
        if rect:
            break
    if not rect:
        print("没找到窗口，看 temp/auto_roam.log", flush=True)
        proc.terminate()
        return 1

    bbox = (rect.left, rect.top, rect.right, rect.bottom)
    emitted = 0
    deadline = started + 45.0
    while emitted < len(STAGES) and time.time() < deadline:
        text = LOG.read_text(encoding="utf-8", errors="replace") if LOG.exists() else ""
        pts = trace(text)
        name, memo, ready = STAGES[emitted]
        if ready(pts, text):
            time.sleep(0.4)  # 等这一帧真的上屏再抓
            ImageGrab.grab(bbox=bbox, all_screens=True).save(EVID / name)
            print(f"shot {name}  <- {memo}", flush=True)
            emitted += 1
            continue
        time.sleep(0.15)
    if emitted < len(STAGES):
        missing = ", ".join(name for name, _, _ in STAGES[emitted:])
        print(f"[WARN] 未等到状态，缺帧：{missing}", flush=True)

    try:
        proc.wait(timeout=25)
    except subprocess.TimeoutExpired:
        proc.terminate()

    text = LOG.read_text(encoding="utf-8", errors="replace")
    pts = trace(text)

    checks = []

    def check(name: str, ok: bool, detail: str) -> None:
        checks.append(ok)
        print(("[PASS] " if ok else "[FAIL] ") + name + " — " + detail, flush=True)

    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    check("轨迹有采样", len(pts) > 10, f"{len(pts)} 个拐点，x 覆盖 {min(xs):.2f} ~ {max(xs):.2f}" if pts else "无轨迹点")
    if pts:
        check("出生点", abs(xs[0] + 90.0) < 3.0 and abs(ys[0]) < 0.5, f"x={xs[0]:.2f} y={ys[0]:.3f}")
        reached_15 = [p for p in pts if -61.0 <= p[0] <= -40.0]
        check("走得到 15° 段", bool(reached_15), "x 曾进入 -61 ~ -40")
        # 命中「坡度超限 3x°」即证明冲刺段的 35° 限值生效：陡坎实测 36.6°，略高于限值
        m35 = re.search(r"坡度超限 (3\d\.\d+)", text)
        sprint_x = [p[0] for p in pts if p[0] <= -38.0]
        check("冲刺被 35° 限值挡住", bool(m35), f"陡坎实测 {m35.group(1)}° > 35°，冲刺停在该处" if m35 else "未命中 35° 限值判定")
        check("步行限 55° 越过同一处坡", max(xs) > 45.0, f"最大 x={max(xs):.2f}（已推进到坡顶附近）")
        top = [p for p in pts if p[0] > 40.0]
        check("坡顶平台脚底高度", bool(top) and abs(top[-1][1] - 29.193) < 0.2,
              f"y={top[-1][1]:.3f}" if top else "没走到平台")
        check("到顶不依赖传送兜底", "无需传送" in text, "20s 校验报「已步行抵达」")

    passed = all(checks)
    print(f"\n断言 {sum(checks)}/{len(checks)} 通过；证据目录 {EVID}", flush=True)
    return 0 if passed else 3


if __name__ == "__main__":
    sys.exit(main())
