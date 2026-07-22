# Raise the Windows GPU TDR (Timeout Detection & Recovery) timeout so RenderDoc can capture Elden Ring
# without DXGI_ERROR_DEVICE_REMOVED.
#
# WHY: RenderDoc's per-resource tracking overhead during ER's ~25s title load makes a single GPU
# operation exceed the DEFAULT 2-second TDR window, so the driver resets the device (TDR) and ER dies at
# the title before it ever reaches the world. Confirmed via RenderDoc's DRED: "No DRED page fault
# information" + "0 DRED nodes" = a timeout, not a bad memory access. Everything else in the RenderDoc
# capture path is already solved (AMD AGS stub lets ER load; the product's Present-overlay is gated off so
# RenderDoc's resource_manager assertion no longer fires). bd RENDERDOC-deviceremoved-is-TDR-needs-TdrDelay.
#
# WHAT THIS DOES: sets TdrDelay + TdrDdiDelay to 60 seconds (from the 2s default). This ONLY affects the
# GPU-hang recovery timeout; normal gameplay is unaffected. A REBOOT is required for it to take effect.
#
# RUN THIS ELEVATED (right-click PowerShell -> Run as administrator, or from an elevated shell):
#     powershell -ExecutionPolicy Bypass -File scripts\renderdoc-set-tdrdelay.ps1
# then REBOOT, then re-run the capture:
#     NO_TRACE=1 RENDERDOC=1 bash scripts/run-samechar-3x-threedll.sh
#
# REVERT (restore the default 2s behaviour):
#     Remove-ItemProperty -Path 'HKLM:\System\CurrentControlSet\Control\GraphicsDrivers' -Name TdrDelay,TdrDdiDelay

$ErrorActionPreference = 'Stop'
$key = 'HKLM:\System\CurrentControlSet\Control\GraphicsDrivers'

# Fail early + clearly if not elevated (writing HKLM needs admin).
$admin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Write-Error 'Not elevated. Re-run this in an ADMINISTRATOR PowerShell (writing HKLM requires admin).'
    exit 1
}

Set-ItemProperty -Path $key -Name TdrDelay    -Type DWord -Value 60
Set-ItemProperty -Path $key -Name TdrDdiDelay -Type DWord -Value 60

Write-Host 'TdrDelay=60 and TdrDdiDelay=60 set at' $key
Write-Host 'REBOOT now for the change to take effect, then re-run the RenderDoc capture.'
