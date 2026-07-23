# Capture ONLY the Elden Ring game window to a PNG (Windows-native, for the WSL2 +
# native-Windows-Steam box where the grim/Hyprland helpers do not apply).
#
# Resolves the window strictly from the running eldenring.exe process (MainWindowHandle),
# validates a sane on-screen rect, then PrintWindow-captures that window (falls back to a
# CopyFromScreen of the window rect). Writes <out>.png on success, or <out>.txt with the
# failure reason and takes no screenshot if the game window can't be validated. Never
# enumerates or captures other windows / the desktop.
#
# Usage: powershell.exe -ExecutionPolicy Bypass -File capture-er-window-win.ps1 <out.png>
param([Parameter(Mandatory=$true)][string]$Out)

$ErrorActionPreference = 'Stop'
$note = [System.IO.Path]::ChangeExtension($Out, '.txt')

function Fail($msg) { Set-Content -Path $note -Value "capture fail: $msg" -Encoding UTF8; exit 0 }

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinCap {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

Add-Type -AssemblyName System.Drawing

$proc = Get-Process -Name eldenring -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not $proc) { Fail "no eldenring.exe process with a main window" }
$h = $proc.MainWindowHandle
if ([WinCap]::IsIconic($h)) { Fail "window minimized" }

$r = New-Object WinCap+RECT
if (-not [WinCap]::GetWindowRect($h, [ref]$r)) { Fail "GetWindowRect failed" }
$w = $r.Right - $r.Left
$hh = $r.Bottom - $r.Top
if ($w -le 0 -or $hh -le 0 -or $w -gt 16384 -or $hh -gt 16384) { Fail "bad geometry ${w}x${hh}" }

$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PrintWindow flag 2 = PW_RENDERFULLCONTENT (captures D3D/DWM-composited client too).
$ok = [WinCap]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
if (-not $ok) {
  # Fallback: copy the window's screen region (works for borderless/windowed, not exclusive fullscreen).
  $g.CopyFromScreen($r.Left, $r.Top, 0, 0, (New-Object System.Drawing.Size $w, $hh))
}
$g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Set-Content -Path $note -Value "capture ok: ${w}x${hh} printwindow=$ok pid=$($proc.Id)" -Encoding UTF8
exit 0
