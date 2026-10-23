@echo off
rem r3e 启动器：优先用 release 构建，没有则现编译。
setlocal
cd /d "%~dp0"
set PATH=C:\Users\neroy\AppData\Local\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin;%USERPROFILE%\.cargo\bin;%PATH%
set RUST_LOG=info

if exist "target\x86_64-pc-windows-gnu\release\r3e.exe" (
    target\x86_64-pc-windows-gnu\release\r3e.exe %*
) else (
    cargo run --release -- %*
)
pause
