<#
.SYNOPSIS
Starts an app, waits, and saves a PNG of its main window only (PrintWindow), then closes it.
Used to check that the benchmark apps really drew their editor and grid.
#>
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $OutPng,
    [int] $WaitSeconds = 4
)
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
$proc = Start-Process -FilePath $Exe -PassThru
Start-Sleep -Seconds $WaitSeconds
$proc.Refresh()
$hwnd = $proc.MainWindowHandle
$r = New-Object Win+RECT
[void][Win]::GetWindowRect($hwnd, [ref]$r)
$bmp = New-Object System.Drawing.Bitmap ($r.Right - $r.Left), ($r.Bottom - $r.Top)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
[void][Win]::PrintWindow($hwnd, $hdc, 2)  # PW_RENDERFULLCONTENT, needed for GPU-drawn windows
$g.ReleaseHdc($hdc)
$bmp.Save($OutPng, [System.Drawing.Imaging.ImageFormat]::Png)
& taskkill.exe /PID $proc.Id /T /F | Out-Null
