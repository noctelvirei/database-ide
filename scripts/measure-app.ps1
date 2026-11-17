#Requires -Version 5.1
# Runs on Windows PowerShell 5.1, which every Windows PC has, so contributors need nothing installed.
<#
.SYNOPSIS
Measures the app against the spec's targets: startup under 1 s, idle private working set
under 100 MB, and the release zipped under 20 MB.

.DESCRIPTION
Each run starts the release exe with DBIDE_FIRST_FRAME set; the app writes the Unix time in
milliseconds of its first frame there. Startup is that time minus the launch time. After
SettleSeconds the script sums private working set (Task Manager's Memory column) over the app
and every process it started, then closes them. Run 1 follows the build, so its startup is
not a warm start; the warm figure is the median of the later runs.

Writes one CSV row per run, then a summary row with the median warm startup, the largest
private working set, the zip size and pass/fail against each target.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [int] $Runs = 6,
    [int] $SettleSeconds = 5
)
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

function Get-ProcessTree([int] $RootId) {
    $all = Get-CimInstance Win32_Process
    $ids = [System.Collections.Generic.List[int]]::new()
    $ids.Add($RootId)
    for ($i = 0; $i -lt $ids.Count; $i++) {
        foreach ($p in $all) {
            if ($p.ParentProcessId -eq $ids[$i] -and -not $ids.Contains([int]$p.ProcessId)) { $ids.Add([int]$p.ProcessId) }
        }
    }
    $ids
}

$exePath = (Resolve-Path -LiteralPath $Exe).Path
$temp = Join-Path ([System.IO.Path]::GetTempPath()) "dbide-measure-$PID"
New-Item -ItemType Directory -Force -Path $temp | Out-Null
$marker = Join-Path $temp 'first-frame.txt'
$rows = @()
try {
    'run,startup_ms,processes,private_ws_mb'
    for ($run = 1; $run -le $Runs; $run++) {
        Remove-Item -LiteralPath $marker -ErrorAction SilentlyContinue
        $env:DBIDE_FIRST_FRAME = $marker
        $t0 = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        $app = Start-Process -FilePath $exePath -PassThru
        $deadline = (Get-Date).AddSeconds(30)
        while (-not (Test-Path -LiteralPath $marker) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 5 }
        $startup = if (Test-Path -LiteralPath $marker) { [int64](Get-Content -LiteralPath $marker -Raw) - $t0 } else { -1 }
        Start-Sleep -Seconds $SettleSeconds
        $ids = Get-ProcessTree $app.Id
        $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $ids -contains [int]$_.IDProcess }
        $privateMb = ($perf | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB
        & taskkill.exe /PID $app.Id /T /F | Out-Null
        $row = [pscustomobject]@{ Run = $run; StartupMs = $startup; Processes = @($ids).Count; PrivateMb = $privateMb }
        $rows += $row
        '{0},{1},{2},{3:N1}' -f $row.Run, $row.StartupMs, $row.Processes, $row.PrivateMb
        Start-Sleep -Seconds 2
    }

    $zip = Join-Path $temp 'database-ide.zip'
    Compress-Archive -LiteralPath $exePath -DestinationPath $zip -CompressionLevel Optimal -Force
    $zipMb = (Get-Item -LiteralPath $zip).Length / 1MB
    $exeMb = (Get-Item -LiteralPath $exePath).Length / 1MB

    $warm = @($rows | Select-Object -Skip 1 | ForEach-Object StartupMs | Sort-Object)
    $middle = [int][math]::Floor($warm.Count / 2)
    $warmMedian = if ($warm.Count -eq 0) { $rows[0].StartupMs }
        elseif ($warm.Count % 2) { $warm[$middle] }
        else { ($warm[$middle - 1] + $warm[$middle]) / 2 }
    $maxPrivate = ($rows | Measure-Object PrivateMb -Maximum).Maximum
    $failed = @($rows | Where-Object StartupMs -lt 0).Count -gt 0
    $pass = { param($ok) if ($ok) { 'pass' } else { 'FAIL' } }
    ''
    'measure,value,target,result'
    'first_run_startup_ms,{0},1000,{1}' -f $rows[0].StartupMs, (& $pass ((-not $failed) -and $rows[0].StartupMs -lt 1000))
    'warm_startup_median_ms,{0},1000,{1}' -f $warmMedian, (& $pass ((-not $failed) -and $warmMedian -lt 1000))
    'max_private_ws_mb,{0:N1},100,{1}' -f $maxPrivate, (& $pass ($maxPrivate -lt 100))
    'exe_mb,{0:N1},,' -f $exeMb
    'zip_mb,{0:N1},20,{1}' -f $zipMb, (& $pass ($zipMb -lt 20))
}
finally {
    Remove-Item Env:DBIDE_FIRST_FRAME -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
