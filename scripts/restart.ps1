# 一键重启 NovelX（Rust web 服务，默认托管 web/dist）— Windows PowerShell
#
# 用法：
#   .\scripts\restart.ps1                 # 编译 CLI + 构建前端，杀旧进程后启动
#   .\scripts\restart.ps1 -Quick          # 不编译，只杀进程重启（需已有二进制与 dist）
#   .\scripts\restart.ps1 -NoWeb          # 编译 CLI，但不重新 npm build
#   .\scripts\restart.ps1 -Release        # 用 release 二进制
#   .\scripts\restart.ps1 -Bind 0.0.0.0:8765
#   .\scripts\restart.ps1 -Daemon         # 后台运行，日志写入 .novelx\web.log
#
# 若执行策略拦截：Set-ExecutionPolicy -Scope CurrentUser RemoteSigned

[CmdletBinding()]
param(
    [switch]$Quick,
    [switch]$NoWeb,
    [switch]$NoBuild,
    [switch]$Release,
    [Alias("d")]
    [switch]$Daemon,
    [string]$Bind = $(if ($env:NOVELX_BIND) { $env:NOVELX_BIND } else { "127.0.0.1:8765" }),
    [switch]$Help
)

$ErrorActionPreference = "Stop"

if ($Help) {
    @"
一键重启 NovelX（Rust web 服务，默认托管 web/dist）

用法：
  .\scripts\restart.ps1                 # 编译 CLI + 构建前端，杀旧进程后启动
  .\scripts\restart.ps1 -Quick          # 不编译，只杀进程重启（需已有二进制与 dist）
  .\scripts\restart.ps1 -NoWeb          # 编译 CLI，但不重新 npm build
  .\scripts\restart.ps1 -Release        # 用 release 二进制
  .\scripts\restart.ps1 -Bind 0.0.0.0:8765
  .\scripts\restart.ps1 -Daemon         # 后台运行，日志写入 .novelx\web.log
"@
    exit 0
}

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

$BuildCli = -not ($Quick -or $NoBuild)
$BuildWeb = -not ($Quick -or $NoBuild -or $NoWeb)

if ($Bind -notmatch ":(\d+)$") {
    Write-Error "无效 -Bind：$Bind（期望 host:port，例如 127.0.0.1:8765）"
}
$Port = [int]$Matches[1]

$BinDir = if ($Release) {
    Join-Path $Root "target\release"
} else {
    Join-Path $Root "target\debug"
}
$ProfileFlag = @()
if ($Release) { $ProfileFlag = @("--release") }
$NovelBin = Join-Path $BinDir "novel.exe"

function Stop-PortListeners {
    param([int]$PortNum)
    $conns = @(Get-NetTCPConnection -LocalPort $PortNum -State Listen -ErrorAction SilentlyContinue)
    if (-not $conns.Count) {
        # Fallback when Get-NetTCPConnection unavailable / needs admin
        $lines = @(netstat -ano | Select-String -Pattern ":$PortNum\s+.*LISTENING")
        $pids = @(
            $lines | ForEach-Object {
                if ($_ -match "\s+(\d+)\s*$") { [int]$Matches[1] }
            } | Select-Object -Unique
        )
        if (-not $pids.Count) {
            Write-Host "→ :$PortNum 无监听进程"
            return
        }
        Write-Host "→ 停止占用 :$PortNum 的进程: $($pids -join ' ')"
        foreach ($procId in $pids) {
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
        Start-Sleep -Milliseconds 400
        return
    }

    $pids = @($conns | Select-Object -ExpandProperty OwningProcess -Unique)
    Write-Host "→ 停止占用 :$PortNum 的进程: $($pids -join ' ')"
    foreach ($procId in $pids) {
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 400

    $left = @(Get-NetTCPConnection -LocalPort $PortNum -State Listen -ErrorAction SilentlyContinue)
    if ($left.Count) {
        $pids = @($left | Select-Object -ExpandProperty OwningProcess -Unique)
        foreach ($procId in $pids) {
            Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
        }
        Start-Sleep -Milliseconds 200
    }
}

Write-Host "== NovelX 重启 =="
Write-Host "  root: $Root"
Write-Host "  bind: $Bind"

Stop-PortListeners -PortNum $Port

if ($BuildCli) {
    Write-Host "→ cargo build -p novelx-cli $($ProfileFlag -join ' ')"
    & cargo build -p novelx-cli @ProfileFlag
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} else {
    Write-Host "→ 跳过 CLI 编译"
}

if (-not (Test-Path -LiteralPath $NovelBin)) {
    Write-Error "找不到可执行文件 $NovelBin`n请先运行: cargo build -p novelx-cli $($ProfileFlag -join ' ')"
}

if ($BuildWeb) {
    Write-Host "→ npm run build (web/)"
    $webDir = Join-Path $Root "web"
    Push-Location $webDir
    try {
        if (-not (Test-Path (Join-Path $webDir "node_modules"))) {
            & npm install
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        }
        & npm run build
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } finally {
        Pop-Location
    }
} else {
    Write-Host "→ 跳过前端构建"
}

$NovelxDir = Join-Path $Root ".novelx"
New-Item -ItemType Directory -Force -Path $NovelxDir | Out-Null
$Log = Join-Path $NovelxDir "web.log"
$PidFile = Join-Path $NovelxDir "web.pid"

Write-Host "→ 启动: $NovelBin web --bind $Bind"
if ($Daemon) {
    # Start-Process cannot attach stdout+stderr to the same path; merge err into log via tee-less pair.
    $ErrLog = Join-Path $NovelxDir "web.err.log"
    foreach ($f in @($Log, $ErrLog)) {
        if (Test-Path -LiteralPath $f) { Remove-Item -LiteralPath $f -Force -ErrorAction SilentlyContinue }
    }
    $proc = Start-Process -FilePath $NovelBin `
        -ArgumentList @("web", "--bind", $Bind) `
        -WorkingDirectory $Root `
        -RedirectStandardOutput $Log `
        -RedirectStandardError $ErrLog `
        -WindowStyle Hidden `
        -PassThru
    Set-Content -LiteralPath $PidFile -Value $proc.Id -Encoding ascii
    Start-Sleep -Milliseconds 500
    if ($proc.HasExited) {
        Write-Host "启动失败，见日志: $Log / $ErrLog" -ForegroundColor Red
        foreach ($f in @($Log, $ErrLog)) {
            if (Test-Path -LiteralPath $f) {
                Get-Content -LiteralPath $f -Tail 40 | Write-Host
            }
        }
        exit 1
    }
    Write-Host "✓ 已后台运行 pid=$($proc.Id)"
    Write-Host "  日志: $Log"
    Write-Host "  打开: http://${Bind}/"
} else {
    Write-Host "✓ 前台运行（Ctrl+C 停止）→ http://${Bind}/"
    & $NovelBin web --bind $Bind
    exit $LASTEXITCODE
}
