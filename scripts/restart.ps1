# 一键：挂载 NovelX 预设、重启阅读台、启动 dsh Web
#
#   .\scripts\restart.ps1
#   .\scripts\restart.ps1 -Quick
#   .\scripts\restart.ps1 -Bind 0.0.0.0:8765
#   .\scripts\restart.ps1 -Daemon

[CmdletBinding()]
param(
    [switch]$Quick,
    [switch]$NoWeb,
    [switch]$NoBuild,
    [switch]$Release,
    [Alias("d")]
    [switch]$Daemon,
    [string]$Bind = $(if ($env:NOVELX_BIND) { $env:NOVELX_BIND } else { "127.0.0.1:8765" }),
    [string]$DshBind = $(if ($env:DSH_BIND) { $env:DSH_BIND } else { "127.0.0.1:3080" }),
    [switch]$Help
)

$ErrorActionPreference = "Stop"
if ($Help) {
    @"
一键挂载 NovelX、重启阅读台、启动 dsh
  .\scripts\restart.ps1
  .\scripts\restart.ps1 -Quick
  .\scripts\restart.ps1 -Bind 0.0.0.0:8765
  .\scripts\restart.ps1 -Daemon
"@
    exit 0
}

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root
$PortNum = [int]($Bind.Split(':')[-1])
$DshPort = [int]($DshBind.Split(':')[-1])
$DshHome = if ($env:DSH_HOME) { $env:DSH_HOME } else { Join-Path $env:USERPROFILE ".dsh" }
$BuildWeb = -not ($Quick -or $NoWeb -or $NoBuild)
if (-not $PSBoundParameters.ContainsKey('Daemon')) { $Daemon = $true }

function Test-PortUp([int]$PortNum) {
    return [bool]@(Get-NetTCPConnection -LocalPort $PortNum -State Listen -ErrorAction SilentlyContinue)
}

function Wait-PortUp([int]$PortNum, [int]$Tries = 80) {
    for ($i = 0; $i -lt $Tries; $i++) {
        if (Test-PortUp -PortNum $PortNum) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}

function Write-Urls {
    Write-Host ""
    Write-Host "打开："
    Write-Host "  阅读台  http://${Bind}/"
    if (Test-PortUp -PortNum $DshPort) {
        Write-Host "  dsh     http://${DshBind}/   （新开会话，预设 NovelX）"
    } else {
        Write-Host "  dsh     http://${DshBind}/   （未起来，见 .novelx/dsh.log）"
    }
}

function Stop-PortListeners([int]$PortNum) {
    $conns = @(Get-NetTCPConnection -LocalPort $PortNum -State Listen -ErrorAction SilentlyContinue)
    if (-not $conns.Count) {
        Write-Host "→ :$PortNum 无监听进程"
        return
    }
    $pids = @($conns | Select-Object -ExpandProperty OwningProcess -Unique)
    Write-Host "→ 停止占用 :$PortNum 的进程: $($pids -join ' ')"
    foreach ($procId in $pids) {
        Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 400
}

function Install-NovelxPreset {
    $src = Join-Path $Root "integrations\dsh-preset-novelx\preset"
    $dest = Join-Path $DshHome ".agent-presets\novelx"
    if (-not (Test-Path (Join-Path $src "preset.yml"))) {
        throw "找不到预设 $src\preset.yml"
    }
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Write-Host "→ 挂载 NovelX 预设 → $dest"
    Copy-Item -Path (Join-Path $src "*") -Destination $dest -Recurse -Force
}

function Ensure-DefaultPreset {
    $file = Join-Path $DshHome "settings.yaml"
    New-Item -ItemType Directory -Force -Path $DshHome | Out-Null
    $text = if (Test-Path $file) { Get-Content -LiteralPath $file -Raw } else { "" }
    if ($text -match '(?m)^agent-presets:\r?\n(?:[ \t].*\r?\n)*[ \t]+default:\s*novelx\s*$') {
        Write-Host "→ dsh 默认预设: novelx"
        return
    }
    if ($text -notmatch '(?m)^agent-presets:') {
        if ($text -and -not $text.EndsWith("`n")) { $text += "`n" }
        $text += "`nagent-presets:`n  default: novelx`n"
    } elseif ($text -match '(?m)^[ \t]+default:') {
        $text = [regex]::Replace($text, '(?m)^([ \t]+)default:.*$', '${1}default: novelx', 1)
    } else {
        $text = $text -replace '(?m)^agent-presets:', "agent-presets:`n  default: novelx"
    }
    Set-Content -LiteralPath $file -Value $text -Encoding utf8
    Write-Host "→ dsh 默认预设: novelx"
}

Write-Host "== NovelX 重启 =="
Write-Host "  root:   $Root"
Write-Host "  阅读台: $Bind"
Write-Host "  dsh:    $DshBind"

Install-NovelxPreset
Ensure-DefaultPreset

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

$script = Join-Path $Root "web\desk-server.mjs"
if (-not (Test-Path -LiteralPath $script)) {
    Write-Error "找不到 $script"
}

$NovelxDir = Join-Path $Root ".novelx"
New-Item -ItemType Directory -Force -Path $NovelxDir | Out-Null
$Log = Join-Path $NovelxDir "web.log"
$PidFile = Join-Path $NovelxDir "web.pid"
$DshLog = Join-Path $NovelxDir "dsh.log"
$DshPidFile = Join-Path $NovelxDir "dsh.pid"
$env:NOVELX_ROOT = $Root
$env:NOVELX_BIND = $Bind

Write-Host "→ 切换阅读台 :$PortNum"
Stop-PortListeners -PortNum $PortNum
Write-Host "→ 启动: node web/desk-server.mjs --bind $Bind"
$deskErr = Join-Path $NovelxDir "web.err.log"
$desk = Start-Process -FilePath "node" `
    -ArgumentList @($script, "--bind", $Bind) `
    -WorkingDirectory $Root `
    -RedirectStandardOutput $Log `
    -RedirectStandardError $deskErr `
    -WindowStyle Hidden `
    -PassThru
Set-Content -LiteralPath $PidFile -Value $desk.Id -Encoding ascii
if (-not (Wait-PortUp -PortNum $PortNum -Tries 20)) {
    Write-Error "阅读台启动失败，见日志: $Log"
}
Write-Host "✓ 阅读台 pid=$($desk.Id)"

Write-Host "→ 切换 dsh :$DshPort"
Stop-PortListeners -PortNum $DshPort
Write-Host "→ 启动: dsh web --port $DshPort  （cwd=$Root，预设 novelx）"
$dshArgs = @("@deepseek-ai/dsh", "web", "--port", "$DshPort")
if ($Daemon) {
    $dshErr = Join-Path $NovelxDir "dsh.err.log"
    $dsh = Start-Process -FilePath "npx" `
        -ArgumentList $dshArgs `
        -WorkingDirectory $Root `
        -RedirectStandardOutput $DshLog `
        -RedirectStandardError $dshErr `
        -WindowStyle Hidden `
        -PassThru
    Set-Content -LiteralPath $DshPidFile -Value $dsh.Id -Encoding ascii
    if (-not (Wait-PortUp -PortNum $DshPort -Tries 120)) {
        Write-Error "dsh 启动失败，见日志: $DshLog"
    }
    Write-Host "✓ dsh pid=$($dsh.Id)"
    Write-Urls
} else {
    Write-Urls
    Write-Host "✓ dsh 前台运行（Ctrl+C 停止）"
    & npx --yes @deepseek-ai/dsh web --port $DshPort
    exit $LASTEXITCODE
}
