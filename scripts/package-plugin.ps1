<#
.SYNOPSIS
把 rust3d-engine 打成 ai_ide 可装载的插件包。

.DESCRIPTION
产物布局（与 ai_ide 的 manifest 校验口径一致）：

    rust3d-engine-plugin-<ver>/
      plugin.json          <- 由 `r3e-plugin --emit-manifest` 现算，不手写
      bin/r3e-plugin.exe   <- 入口（plugin.json 的 main 指向这里）
      examples/*.json      <- 可直接喂给 scene_inspect / physics_simulate 的示例场景
      README.md            <- 挂载与调用说明

打完包会拿**包内那份 exe** 跑一次 `--smoke-test`（自带最小宿主的端到端自检），
自检不过就不会产出 zip，避免把装不上的包当成分发物。

.PARAMETER Features
额外开启的 cargo feature，例如 `rapier`（把 Rapier 物理后端编进插件）。

.PARAMETER SkipSmoke
跳过端到端自检（仅供调试；正式产物不要用）。

.EXAMPLE
pwsh scripts/package-plugin.ps1
pwsh scripts/package-plugin.ps1 -Features rapier
#>
[CmdletBinding()]
param(
    [string]$Features = "",
    [string]$OutDir = "dist",
    [switch]$SkipSmoke
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
# 原生 exe 的 stdout 是 UTF-8；Windows PowerShell 5.1 默认按控制台代码页（本机 GBK）解码，
# 会把清单里的中文解成乱码，后面 ConvertFrom-Json 直接抛 ArgumentException。
# 强制 UTF-8 之后，`& $exe --emit-manifest` 拿到才是可解析的 JSON。
$previousOutputEncoding = [Console]::OutputEncoding
try { [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false) } catch { }
try {
    # cargo 不一定在 PATH 里（本机实测默认只有 MinGW 的 bin）
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if ((Test-Path $cargoBin) -and ($env:PATH -notlike "*$cargoBin*")) {
        $env:PATH = "$cargoBin;$env:PATH"
    }

    # ---- 版本号：以 workspace.package.version 为唯一来源 ----
    $rootToml = Get-Content (Join-Path $repoRoot "Cargo.toml") -Raw -Encoding UTF8
    if ($rootToml -notmatch '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"') {
        throw "没在 Cargo.toml 的 [workspace.package] 里找到 version"
    }
    $version = $Matches[1]

    $pkgName = "rust3d-engine-plugin-$version"
    $stageRoot = Join-Path $repoRoot $OutDir
    $stage = Join-Path $stageRoot $pkgName
    $zipPath = Join-Path $stageRoot "$pkgName.zip"

    Write-Host "[package] 版本 $version，产物目录 $stage"

    # ---- 1. 构建 ----
    $cargoArgs = @("build", "--release", "-p", "r3e-plugin")
    if ($Features) { $cargoArgs += @("--features", $Features) }
    Write-Host "[package] cargo $($cargoArgs -join ' ')"
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（RC=$LASTEXITCODE）" }

    # 带 feature 构建时产物在 target\<triple>\release，不带时在 target\release
    $exeName = "r3e-plugin.exe"
    $candidates = @(
        (Join-Path $repoRoot "target\release\$exeName"),
        (Join-Path $repoRoot "target\x86_64-pc-windows-gnu\release\$exeName")
    )
    $exe = $candidates | Where-Object { Test-Path $_ } |
        Sort-Object { (Get-Item $_).LastWriteTime } -Descending |
        Select-Object -First 1
    if (-not $exe) { throw "找不到构建产物：$($candidates -join ' / ')" }
    Write-Host "[package] 二进制 $exe"

    # ---- 2. 搭包 ----
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    New-Item -ItemType Directory -Path (Join-Path $stage "bin") -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $stage "examples") -Force | Out-Null

    Copy-Item $exe (Join-Path $stage "bin\$exeName") -Force
    $packedExe = Join-Path $stage "bin\$exeName"

    # ---- 3. 清单：由二进制现算，保证与握手回包同源 ----
    $manifestPath = Join-Path $stage "plugin.json"
    $manifestText = & $packedExe --emit-manifest
    if ($LASTEXITCODE -ne 0) { throw "生成 plugin.json 失败（RC=$LASTEXITCODE）" }
    [System.IO.File]::WriteAllText($manifestPath, ($manifestText -join "`n") + "`n", (New-Object System.Text.UTF8Encoding($false)))

    $manifest = Get-Content $manifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $entry = [string]$manifest.main -replace '/', '\'
    if (-not (Test-Path (Join-Path $stage $entry))) {
        throw "清单里的 main=$($manifest.main) 在包内不存在，宿主装载会报 6108"
    }
    Write-Host ("[package] 清单 id={0} api={1} 工具={2} 命令={3} 能力={4}" -f `
        $manifest.id, $manifest.pluginApi, $manifest.tools.Count, $manifest.commands.Count, ($manifest.capabilities -join ","))

    # ---- 4. 示例场景 + 文档 ----
    $examples = @("scene.json", "scene_physics.json", "scene_rapier.json", "scene_ccd.json")
    foreach ($name in $examples) {
        $src = Join-Path $repoRoot "assets\$name"
        if (Test-Path $src) { Copy-Item $src (Join-Path $stage "examples\$name") -Force }
    }
    $readme = Join-Path $repoRoot "docs\plugin-packaging.md"
    if (Test-Path $readme) { Copy-Item $readme (Join-Path $stage "README.md") -Force }

    # ---- 5. 端到端自检（跑包内那份 exe） ----
    if (-not $SkipSmoke) {
        Write-Host "[package] 跑 --smoke-test（真实子进程 + 完整协议）"
        $log = Join-Path $env:TEMP "r3e-plugin-smoke.log"
        # 走 cmd 而不是直接 `& $exe *> $log`：PowerShell 5.1 会把原生命令的 stderr
        # 包成 NativeCommandError 错误记录混进日志（自检日志全在 stderr），
        # 看上去像失败。cmd 的 `> ... 2>&1` 只做字节级重定向，退出码照旧回填到 $LASTEXITCODE。
        # 也不使用 Start-Process：它的 -PassThru ExitCode 在 5.1 里经常回填不出来。
        cmd /c "`"$packedExe`" --smoke-test > `"$log`" 2>&1"
        $exitCode = $LASTEXITCODE
        Get-Content $log -Encoding UTF8 | ForEach-Object { Write-Host "  $_" }
        if ($exitCode -ne 0) { throw "自检失败（RC=$exitCode），不产出分发包" }
    } else {
        Write-Warning "已跳过自检：该产物不应对外分发"
    }

    # ---- 6. 压缩 + 校验和 ----
    Compress-Archive -Path $stage -DestinationPath $zipPath -CompressionLevel Optimal -Force
    $zipHash = (Get-FileHash $zipPath -Algorithm SHA256).Hash
    $exeHash = (Get-FileHash $packedExe -Algorithm SHA256).Hash
    $sumsPath = Join-Path $stageRoot "SHA256SUMS.txt"
    @(
        "$zipHash  $pkgName.zip",
        "$exeHash  $pkgName/bin/r3e-plugin.exe"
    ) | Set-Content $sumsPath -Encoding UTF8

    $zipSize = [math]::Round((Get-Item $zipPath).Length / 1MB, 2)
    $exeSize = [math]::Round((Get-Item $packedExe).Length / 1MB, 2)
    Write-Host ""
    Write-Host "[package] 完成"
    Write-Host ("  包目录 : {0}" -f $stage)
    Write-Host ("  压缩包 : {0} ({1} MB)" -f $zipPath, $zipSize)
    Write-Host ("  入口   : bin/r3e-plugin.exe ({0} MB)" -f $exeSize)
    Write-Host ("  SHA256 : {0}" -f $zipHash)
}
finally {
    try { [Console]::OutputEncoding = $previousOutputEncoding } catch { }
    Pop-Location
}
