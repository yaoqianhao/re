"""真窗口地形漫游演示：冲刺跑过平地 -> 15° 缓坡 -> 撞上 50° 陡坡被挡 -> 传送坡顶 -> 俯视全景。

前置：先 cargo build（脚本自己会检查 exe 存在），且**必须 rapier 后端**：
高度场只有真物理后端才建成 collider，`physics_raycast` 才打得中地面。

关键前提（stairs 那边的老坑）：SetForegroundWindow 会被前台锁定拒绝，必须先
AttachThreadInput 到当前前台线程再置前并回读校验，否则 keybd_event 打不到引擎窗口。
"""
import ctypes
import ctypes.wintypes as wt
import subprocess
import sys
import time
from pathlib import Path

from PIL import ImageGrab

LAB = Path(__file__).resolve().parent.parent   # assets/labs/terrain_roam
ROOT = LAB.parents[2]                          # 项目根
LOG = ROOT / "temp" / "roam_demo.log"

user32 = ctypes.WinDLL("user32", use_last_error=True)
KEYEVENTF_KEYUP = 0x0002
VK = {"w": 0x57, "a": 0x41, "s": 0x53, "d": 0x44,
      "shift": 0x10, "c": 0x43, "r": 0x52, "t": 0x54}


def force_foreground(hwnd: int) -> bool:
    fg = user32.GetForegroundWindow()
    fg_thread = user32.GetWindowThreadProcessId(fg, None)
    cur = ctypes.windll.kernel32.GetCurrentThreadId()
    attached = False
    if fg_thread and fg_thread != cur:
        attached = bool(user32.AttachThreadInput(fg_thread, cur, True))
    user32.ShowWindow(hwnd, 9)
    user32.BringWindowToTop(hwnd)
    user32.SetForegroundWindow(hwnd)
    user32.SetFocus(hwnd)
    time.sleep(0.25)
    ok = user32.GetForegroundWindow() == hwnd
    if attached:
        user32.AttachThreadInput(fg_thread, cur, False)
    return ok


def find_r3e(pid: int):
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
            hits.append((hwnd, rect))
            return False
        return True

    user32.EnumWindows(cb, 0)
    return hits[0] if hits else None


def hold(keys, seconds: float) -> None:
    for k in keys:
        user32.keybd_event(VK[k], 0, 0, 0)
    time.sleep(seconds)
    for k in keys:
        user32.keybd_event(VK[k], 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.25)


def tap(key: str) -> None:
    user32.keybd_event(VK[key], 0, 0, 0)
    time.sleep(0.12)
    user32.keybd_event(VK[key], 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.35)


def main() -> int:
    exe = ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe"
    if not exe.exists():
        print(f"缺少 {exe}，先 cargo build", flush=True)
        return 2
    (LAB / "evidence").mkdir(exist_ok=True)
    LOG.parent.mkdir(exist_ok=True)
    args = [str(exe), str(LAB / "terrain_roam.r3e.json"), str(LAB / "scripts" / "roam.rhai"),
            "--physics", "on", "--physics-backend", "rapier",
            "--window-size", "960x540", "--run-seconds", "60"]
    proc = subprocess.Popen(args, cwd=str(ROOT),
                            stdout=open(LOG, "w", encoding="utf-8", errors="replace"),
                            stderr=subprocess.STDOUT)
    win = None
    for _ in range(60):
        time.sleep(0.25)
        win = find_r3e(proc.pid)
        if win:
            break
    if not win:
        print("没找到窗口，看 temp/roam_demo.log", flush=True)
        proc.terminate()
        return 1
    hwnd, rect = win
    print("foreground ok:", force_foreground(hwnd), flush=True)
    time.sleep(0.8)

    def shot(name: str) -> None:
        ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom),
                       all_screens=True).save(LAB / "evidence" / name)
        print("shot", name, flush=True)

    shot("roam_00_spawn.png")            # x = -90 平地出生
    hold(["shift", "d"], 4.0)            # 冲刺 8 m/s * 4s ≈ 32 m -> x ≈ -58，15° 坡脚
    shot("roam_01_slope15.png")
    hold(["shift", "d"], 2.6)            # 再 ≈ 21 m -> x ≈ -37，15° 坡顶 / 50° 坡脚
    shot("roam_02_slope50_blocked.png")
    hold(["d"], 1.6)                     # 不冲刺硬顶 50° 坡，验证被挡
    shot("roam_03_hold_blocked.png")
    tap("t")                             # 传送到坡顶平台
    time.sleep(0.6)
    shot("roam_04_top_plateau.png")
    tap("c")                             # 高位俯视，看整张 200×200 地形与 16 块分缝
    shot("roam_05_overview.png")
    try:
        proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        proc.terminate()
    return 0


if __name__ == "__main__":
    sys.exit(main())
