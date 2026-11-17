#Requires -Version 5.1
# Runs on Windows PowerShell 5.1, which every Windows PC has, so contributors need nothing installed.
<#
.SYNOPSIS
Plays a UI script in the app and saves its state lines and screenshots to an artifact folder.

.DESCRIPTION
Starts the app with --ui-script, --ui-state and --ui-shots. The app plays the script through
gpui's own keystroke dispatch (CODE_STANDARDS.md, "UI E2E harness") and appends one JSON line
of visible state per step. At a screenshot step the app writes "<state>.shot" holding the
shot's name; this script saves a PNG of the app's window with PrintWindow (the window need
not be in front) and deletes the marker. No input goes through the operating system.

Outputs, in -OutDir: state.jsonl and one PNG per shot. A rerun overwrites them.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Script,
    [Parameter(Mandatory)] [string] $OutDir,
    [int] $TimeoutSeconds = 120
)
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class UiRunWin {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
'@

function Save-WindowShot([IntPtr] $Handle, [string] $Path) {
    $rect = New-Object UiRunWin+RECT
    [void][UiRunWin]::GetWindowRect($Handle, [ref]$rect)
    $bitmap = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT: needed to capture a GPU-drawn window.
    [void][UiRunWin]::PrintWindow($Handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$state = Join-Path $OutDir 'state.jsonl'
$marker = "$state.shot"
$done = "$state.done"

$argList = @('--ui-script', (Resolve-Path -LiteralPath $Script).Path, '--ui-state', $state, '--ui-shots')
$app = Start-Process -FilePath $Exe -ArgumentList $argList -PassThru
try {
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not (Test-Path -LiteralPath $done) -and -not $app.HasExited) {
        if ((Get-Date) -gt $deadline) { throw "The UI script did not finish within $TimeoutSeconds s." }
        if (Test-Path -LiteralPath $marker) {
            $name = (Get-Content -LiteralPath $marker -Raw).Trim()
            $app.Refresh()
            Save-WindowShot $app.MainWindowHandle (Join-Path $OutDir "$name.png")
            Remove-Item -LiteralPath $marker
        }
        Start-Sleep -Milliseconds 50
    }
    if (-not (Test-Path -LiteralPath $done)) { throw "The app exited before the script finished (exit code $($app.ExitCode))." }
    Get-Content -LiteralPath $state
}
finally {
    if (-not $app.HasExited) { & taskkill.exe /PID $app.Id /T /F | Out-Null }
}
