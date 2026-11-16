<#
.SYNOPSIS
PROTOTYPE. Saves a screenshot of each plan view variant, with each sample plan, into -OutDir.

.DESCRIPTION
Starts the exe once per shot with the flags that set up that shot, waits for it to draw,
then copies only the window's client area with PrintWindow, which works whether or not the
window is in front. No keys or clicks are sent, so nothing reaches other windows.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $OutDir,
    [int] $WaitSeconds = 5
)
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr hwnd, ref POINT p);
    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct POINT { public int X, Y; }
}
"@

function Save-ClientShot([IntPtr] $hwnd, [string] $path) {
    $w = New-Object Win+RECT
    $c = New-Object Win+RECT
    $origin = New-Object Win+POINT
    [void][Win]::GetWindowRect($hwnd, [ref]$w)
    [void][Win]::GetClientRect($hwnd, [ref]$c)
    [void][Win]::ClientToScreen($hwnd, [ref]$origin)
    $full = New-Object System.Drawing.Bitmap ($w.Right - $w.Left), ($w.Bottom - $w.Top)
    $g = [System.Drawing.Graphics]::FromImage($full)
    $hdc = $g.GetHdc()
    [void][Win]::PrintWindow($hwnd, $hdc, 2)  # PW_RENDERFULLCONTENT, needed for GPU-drawn windows
    $g.ReleaseHdc($hdc)
    $crop = New-Object System.Drawing.Rectangle ($origin.X - $w.Left), ($origin.Y - $w.Top), $c.Right, $c.Bottom
    $client = $full.Clone($crop, $full.PixelFormat)
    $client.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    "{0}: {1}x{2}" -f (Split-Path $path -Leaf), $client.Width, $client.Height
}

$shots = @(
    @{ Name = 'a-graph-sqlserver'; Args = @('--variant', 'a', '--plan', '0') },
    @{ Name = 'b-table-sqlserver'; Args = @('--variant', 'b', '--plan', '0') },
    @{ Name = 'c-icicle-sqlserver'; Args = @('--variant', 'c', '--plan', '0') },
    @{ Name = 'a-graph-postgres'; Args = @('--variant', 'a', '--plan', '2') },
    @{ Name = 'b-table-postgres'; Args = @('--variant', 'b', '--plan', '2') },
    @{ Name = 'c-icicle-postgres'; Args = @('--variant', 'c', '--plan', '2') },
    @{ Name = 'misestimate-a-graph-postgres'; Args = @('--variant', 'a', '--plan', '2', '--select', '6') },
    @{ Name = 'misestimate-b-table-sqlserver-keylookup'; Args = @('--variant', 'b', '--plan', '1', '--select', '15') },
    @{ Name = 'misestimate-c-icicle-sqlserver'; Args = @('--variant', 'c', '--plan', '1', '--select', '15') }
)
New-Item -ItemType Directory -Force $OutDir | Out-Null
foreach ($shot in $shots) {
    $proc = Start-Process -FilePath $Exe -ArgumentList $shot.Args -PassThru
    try {
        Start-Sleep -Seconds $WaitSeconds
        $proc.Refresh()
        Save-ClientShot $proc.MainWindowHandle (Join-Path $OutDir "$($shot.Name).png")
    }
    finally {
        if (-not $proc.HasExited) { & taskkill.exe /PID $proc.Id /T /F | Out-Null }
    }
}
