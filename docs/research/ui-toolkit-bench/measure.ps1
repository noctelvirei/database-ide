<#
.SYNOPSIS
Measures time to first frame and memory of a hello-world GUI app, counting every process in its tree.

.DESCRIPTION
Each run starts the app with BENCH_FIRST_FRAME set; the app writes the epoch ms of its first frame there.
After SettleSeconds the script sums, over the app and all its descendants (msedgewebview2.exe included):
  WorkingSet       - Win32_Process.WorkingSetSize (shared pages counted once per process)
  PrivateWS        - Working Set - Private (what Task Manager's Memory column shows)
  Commit           - Win32_Process.PrivatePageCount (private bytes)
Then it kills the tree. Writes one CSV row per run to stdout.
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [int] $Runs = 5,
    [int] $SettleSeconds = 5,
    # Also print private working set per process.
    [switch] $Detail
)

function Get-Tree([int] $RootId) {
    $all = Get-CimInstance Win32_Process
    $ids = [System.Collections.Generic.List[int]]::new()
    $ids.Add($RootId)
    $i = 0
    while ($i -lt $ids.Count) {
        $parent = $ids[$i]
        foreach ($p in $all) { if ($p.ParentProcessId -eq $parent -and -not $ids.Contains([int]$p.ProcessId)) { $ids.Add([int]$p.ProcessId) } }
        $i++
    }
    $all | Where-Object { $ids.Contains([int]$_.ProcessId) }
}

$name = [System.IO.Path]::GetFileNameWithoutExtension($Exe)
"app,run,startup_ms,processes,working_set_mb,private_ws_mb,commit_mb,process_tree"
for ($run = 1; $run -le $Runs; $run++) {
    $marker = Join-Path $env:TEMP "bench-first-frame-$name.txt"
    Remove-Item $marker -ErrorAction SilentlyContinue
    $env:BENCH_FIRST_FRAME = $marker
    $t0 = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $proc = Start-Process -FilePath $Exe -PassThru
    $deadline = (Get-Date).AddSeconds(30)
    while (-not (Test-Path $marker) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 5 }
    $startup = if (Test-Path $marker) { [int64](Get-Content $marker -Raw) - $t0 } else { -1 }
    Start-Sleep -Seconds $SettleSeconds

    $tree = Get-Tree $proc.Id
    $treeIds = $tree | ForEach-Object { [int]$_.ProcessId }
    $perf = Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $treeIds -contains [int]$_.IDProcess }
    $ws = ($tree | Measure-Object WorkingSetSize -Sum).Sum / 1MB
    $pws = ($perf | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB
    $commit = ($tree | Measure-Object PrivatePageCount -Sum).Sum / 1MB
    $names = ($tree | Group-Object Name | ForEach-Object { "$($_.Name) x$($_.Count)" }) -join ' + '
    "{0},{1},{2},{3},{4:N1},{5:N1},{6:N1},{7}" -f $name, $run, $startup, @($tree).Count, $ws, $pws, $commit, $names

    if ($Detail) {
        foreach ($p in $perf) { "  detail,{0},{1},private_ws_mb={2:N1}" -f $p.Name, $p.IDProcess, ($p.WorkingSetPrivate / 1MB) }
    }

    & taskkill.exe /PID $proc.Id /T /F | Out-Null
    Start-Sleep -Seconds 2
}
