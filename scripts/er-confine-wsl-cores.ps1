# Elevated (admin) helper: confine the WSL2 VM host processes (vmmemWSL, vmwp) -- which run ALL the
# parallel cargo/agent threads -- to CPU cores 0-11 and lower their priority, freeing physical cores
# 12-15 for Elden Ring (a native Windows process pinned to 12-15 at High by scripts/boost-er-priority.sh).
# This physically isolates ER from the cargo contention so the load2/load3 movement + FPS proof is
# deterministic. Run elevated (Start-Process -Verb RunAs). Writes results to C:\Windows\Temp so WSL can
# read them at /mnt/c/Windows/Temp/er_vm_confine.log.
$log = "C:\Windows\Temp\er_vm_confine.log"
"---- er-confine-wsl-cores run ----" | Out-File -FilePath $log -Append -Encoding ascii
$mask = [IntPtr]4095   # 0x0FFF = cores 0-11
Get-Process vmmemWSL, vmwp, wslservice -ErrorAction SilentlyContinue | ForEach-Object {
    $n = $_.Name; $id = $_.Id
    try { $_.ProcessorAffinity = $mask } catch { "$n PID=$id affinity FAIL: $($_.Exception.Message)" | Out-File -FilePath $log -Append -Encoding ascii }
    try { $_.PriorityClass = 'BelowNormal' } catch { "$n PID=$id prio FAIL: $($_.Exception.Message)" | Out-File -FilePath $log -Append -Encoding ascii }
    try { "$n PID=$id NOW aff=$($_.ProcessorAffinity) prio=$($_.PriorityClass)" | Out-File -FilePath $log -Append -Encoding ascii } catch {}
}
"---- done ----" | Out-File -FilePath $log -Append -Encoding ascii

# Show the result in THIS elevated window and keep it open so the UAC/password step is unhurried and the
# outcome is visible. The window stays until you press Enter.
Write-Host ""
Write-Host "==== WSL VM core-confinement result (also logged to $log) ===="
Get-Content $log -Tail 12 | Write-Host
Write-Host ""
Read-Host "Confinement applied. Press Enter to close this admin window"
