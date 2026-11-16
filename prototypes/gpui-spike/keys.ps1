<#
.SYNOPSIS
PROTOTYPE. Runs the spike's built-in keyboard script and saves screenshots at the steps that ask for one.

.DESCRIPTION
Starts the exe with SPIKE_KEYSCRIPT set. The app plays the script in src/keyscript.rs through
gpui's own keystroke dispatch and appends one JSON line of state per step to -Out. When a step
asks for a screenshot the app writes "<Out>.shot" with its name; this script saves a PNG of the
window (PrintWindow, so the window does not need to be in front) and deletes the marker.
No keys are sent through the operating system, so nothing reaches other windows.
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Out,
    [string] $ShotDir = ""
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
    public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

function Save-Shot([IntPtr] $hwnd, [string] $path) {
    $r = New-Object Win+RECT
    [void][Win]::GetWindowRect($hwnd, [ref]$r)
    $bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [void][Win]::PrintWindow($hwnd, $hdc, 2)  # PW_RENDERFULLCONTENT, needed for GPU-drawn windows
    $g.ReleaseHdc($hdc)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
}

$marker = "$Out.shot"
$done = "$Out.done"
Remove-Item $Out, $marker, $done -ErrorAction SilentlyContinue
$env:SPIKE_KEYSCRIPT = $Out
$proc = Start-Process -FilePath $Exe -PassThru
try {
    $deadline = (Get-Date).AddMinutes(3)
    while (-not (Test-Path $done) -and (Get-Date) -lt $deadline -and -not $proc.HasExited) {
        if (Test-Path $marker) {
            $name = (Get-Content $marker -Raw).Trim()
            $proc.Refresh()
            if ($ShotDir) { Save-Shot $proc.MainWindowHandle (Join-Path $ShotDir "$name.png") }
            Remove-Item $marker
        }
        Start-Sleep -Milliseconds 50
    }
    Get-Content $Out
}
finally {
    if (-not $proc.HasExited) { & taskkill.exe /PID $proc.Id /T /F | Out-Null }
    Remove-Item Env:SPIKE_KEYSCRIPT
}
