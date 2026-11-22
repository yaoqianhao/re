"""全景静帧采集：起真窗口跑 panorama.rhai，等相机稳定后抓窗口位图存 PNG。"""
import ctypes
import ctypes.wintypes as wt
import subprocess
import time
from pathlib import Path

from PIL import ImageGrab

LAB = Path(__file__).resolve().parent.parent   # assets/labs/character_stairs
ROOT = LAB.parents[2]                          # 项目根 D:\rust3d-engine
USER32 = ctypes.WinDLL("user32", use_last_error=True)


def force_foreground(hwnd: int) -> None:
    fg = USER32.GetForegroundWindow()
    fg_thread = USER32.GetWindowThreadProcessId(fg, None)
    cur = ctypes.windll.kernel32.GetCurrentThreadId()
    attached = False
    if fg_thread and fg_thread != cur:
        attached = bool(USER32.AttachThreadInput(fg_thread, cur, True))
    USER32.ShowWindow(hwnd, 9)
    USER32.BringWindowToTop(hwnd)
    USER32.SetForegroundWindow(hwnd)
    time.sleep(0.3)
    if attached:
        USER32.AttachThreadInput(fg_thread, False)


def find_r3e(pid: int):
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        wpid = wt.DWORD()
        USER32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value != pid or not USER32.IsWindowVisible(hwnd):
            return True
        cls = ctypes.create_unicode_buffer(256)
        USER32.GetClassNameW(hwnd, cls, 256)
        rect = wt.RECT()
        USER32.GetWindowRect(hwnd, ctypes.byref(rect))
        if cls.value == "Window Class" and rect.right - rect.left > 300:
            hits.append((hwnd, rect))
            return False
        return True

    USER32.EnumWindows(cb, 0)
    return hits[0] if hits else None


def main() -> int:
    exe = str(ROOT / "target" / "x86_64-pc-windows-gnu" / "debug" / "r3e.exe")
    args = [exe, str(LAB / "stairs_lab.r3e.json"), str(LAB / "scripts" / "panorama.rhai"),
            "--physics", "on", "--physics-backend", "rapier",
            "--window-size", "1280x720", "--run-seconds", "8"]
    proc = subprocess.Popen(args, cwd=str(ROOT),
                            stdout=open(ROOT / "temp" / "panorama_shot.log", "w",
                                        encoding="utf-8", errors="replace"),
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
    force_foreground(hwnd)
    time.sleep(2.0)
    ImageGrab.grab(bbox=(rect.left, rect.top, rect.right, rect.bottom),
                   all_screens=True).save(LAB / "evidence" / "shot_panorama_overview.png")
    print("saved", (LAB / "evidence" / "shot_panorama_overview.png").stat().st_size, flush=True)
    try:
        proc.wait(timeout=15)
    except subprocess.TimeoutExpired:
        proc.terminate()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
