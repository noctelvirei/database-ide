<#
.SYNOPSIS
PROTOTYPE. Scrolls the spike's 1M-row grid end to end by itself and reports frame times and memory.

.DESCRIPTION
Starts the exe with SPIKE_AUTOSCROLL set. One second after the first frame the app moves the grid
one step per frame from row 0 to the last row (see src/autoscroll.rs for -Mode), recording the
interval between frames, then writes p50/p95/p99/max to a JSON file. Memory is measured the same
way as measure.ps1: idle before the scroll starts, and again 3 s after it ends.
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [ValidateSet('jump', 'wheel', 'drag')] [string] $Mode = 'jump',
    # Rows per frame (jump, wheel) or pixels per frame (drag); 0 uses the app's default.
    [int] $Step = 0
)
$ErrorActionPreference = "Stop"

function Get-Memory([int] $RootId) {
    $p = Get-CimInstance Win32_Process -Filter "ProcessId = $RootId"
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { [int]$_.IDProcess -eq $RootId }
    "working_set_mb={0:N1} private_ws_mb={1:N1} commit_mb={2:N1}" -f ($p.WorkingSetSize / 1MB), ($perf.WorkingSetPrivate / 1MB), ($p.PrivatePageCount / 1MB)
}

$stats = Join-Path $env:TEMP "gpui-spike-scroll.json"
$marker = Join-Path $env:TEMP "gpui-spike-first-frame.txt"
Remove-Item $stats, $marker -ErrorAction SilentlyContinue
$env:SPIKE_AUTOSCROLL = $stats
$env:SPIKE_AUTOSCROLL_MODE = $Mode
if ($Step -gt 0) { $env:SPIKE_AUTOSCROLL_STEP = "$Step" } else { Remove-Item Env:SPIKE_AUTOSCROLL_STEP -ErrorAction SilentlyContinue }
$env:BENCH_FIRST_FRAME = $marker
$proc = Start-Process -FilePath $Exe -PassThru
try {
    while (-not (Test-Path $marker)) { Start-Sleep -Milliseconds 20 }
    Start-Sleep -Milliseconds 900
    "before scroll: $(Get-Memory $proc.Id)"
    $deadline = (Get-Date).AddMinutes(10)
    while (-not (Test-Path $stats) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    "frames: $((Get-Content $stats -Raw).Trim())"
    Start-Sleep -Seconds 3
    "after scroll: $(Get-Memory $proc.Id)"
}
finally {
    & taskkill.exe /PID $proc.Id /T /F | Out-Null
    Remove-Item Env:SPIKE_AUTOSCROLL, Env:SPIKE_AUTOSCROLL_MODE, Env:SPIKE_AUTOSCROLL_STEP -ErrorAction SilentlyContinue
}
