"""真窗口键盘演示：W 上 0.10m 楼梯 -> D 横移到 25 度斜坡车道 -> W 爬坡，分步截图。

关键前提（上一轮踩过）：SetForegroundWindow 会被前台锁定拒绝，必须先
AttachThreadInput 到当前前台线程再置前，并回读 GetForegroundWindow 校验；
否则 keybd_event 打不到引擎窗口，日志里玩家全程不动。
"""
import ctypes
import ctypes.wintypes as wt
import subprocess
import time
from pathlib import Path

from PIL import ImageGrab

LAB = Path(__file__).resolve().parent.parent   # assets/labs/character_stairs
ROOT = LAB.parents[2]                          # 项目根 D:\rust3d-engine
LOG = ROOT / "temp" / "kbd_demo.log"

user32 = ctypes.WinDLL("user32", use_last_error=True)
KEYEVENTF_KEYUP = 0x0002
VK = {"w": 0x57, "a": 0x41, "s": 0x53, "d": 0x44}


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


def hold(key: str, seconds: float) -> None:
    vk = VK[key]
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(seconds)
    user32.keybd_event(vk, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.25)


def main() -> int:
    exe = str(ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe")
    args = [exe, str(LAB / "stairs_lab.r3e.json"), str(LAB / "scripts" / "keyboard_probe.rhai"),
            "--physics", "on", "--physics-backend", "rapier",
            "--window-size", "960x540", "--run-seconds", "20"]
    proc = subprocess.Popen(args, cwd=str(ROOT),
                            stdout=open(LOG, "w", encoding="utf-8", errors="replace"),
                            stderr=subprocess.STDOUT)
    win = None
    for _ in range(40):
        time.sleep(0.25)
        win = find_r3e(proc.pid)
        if win:
            break
    if not win:
        print("没找到窗口", flush=True)
        proc.terminate()
        return 1
    hwnd, rect = win
    print("foreground ok:", force_foreground(hwnd), flush=True)
    time.sleep(0.8)

    def shot(name: str) -> None:
        ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom),
                       all_screens=True).save(LAB / "evidence" / name)
        print("shot", name, flush=True)

    shot("kbd_00_start.png")
    hold("d", 5.7)                      # 先在平地横移 6 条赛道到 x≈8（25 度斜坡车道），避免撞邻道台阶侧壁
    shot("kbd_01_lane_d.png")
    hold("w", 3.4)                      # 爬 25 度斜坡
    shot("kbd_02_ramp_w.png")
    hold("s", 1.6)                      # 倒退下坡，验证下坡方向
    shot("kbd_03_ramp_s.png")
    try:
        proc.wait(timeout=25)
    except subprocess.TimeoutExpired:
        proc.terminate()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
