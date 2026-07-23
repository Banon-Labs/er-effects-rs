param(
  [string]$Destination = "X:\Documents\me3 profiles\er_net_effects_dll.dll",
  [string]$Profile = "X:\Documents\me3 profiles\er_effects_rs.me3",
  [string]$GameDir = "C:\SteamLibrary\steamapps\common\ELDEN RING\Game",
  [string]$SeamlessCoopDllPath = "C:\SteamLibrary\steamapps\common\ELDEN RING\Game\SeamlessCoop\ersc.dll",
  [switch]$ClearCrashTelemetry,
  [switch]$NoCopy
)

$ErrorActionPreference = "Stop"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$source = Join-Path $repoRoot "target\x86_64-pc-windows-msvc\release\er_net_effects_dll.dll"

if (-not (Test-Path -LiteralPath $source)) {
  throw "missing built DLL: $source"
}
if (-not $NoCopy) {
  Copy-Item -LiteralPath $source -Destination $Destination -Force
}
if ($ClearCrashTelemetry) {
  foreach ($name in @(
    "er-net-effects-crash-telemetry-latest.txt",
    "er-net-effects-crash-telemetry.log",
    "er-net-effects-breadcrumb-latest.txt"
  )) {
    $path = Join-Path $GameDir $name
    if (Test-Path -LiteralPath $path) {
      Remove-Item -LiteralPath $path -Force
    }
  }
}

$content = Get-Content -LiteralPath $Profile -Raw
$expectedLine = "path = 'X:\Documents\me3 profiles\er_net_effects_dll.dll'"
$expectedSeamlessLine = "path = '$SeamlessCoopDllPath'"
if (-not $content.Contains($expectedSeamlessLine)) {
  throw "profile is missing quoted game-installed Seamless Co-op native entry: $Profile"
}
if (-not $content.Contains($expectedLine)) {
  throw "profile is missing quoted er_net_effects_dll native entry: $Profile"
}
$seamlessItem = Get-Item -LiteralPath $SeamlessCoopDllPath
$item = Get-Item -LiteralPath $Destination
$hash = Get-FileHash -Algorithm SHA256 -LiteralPath $Destination
$count = ([regex]::Matches($content, [regex]::Escape('er_net_effects_dll.dll'))).Count
$seamlessCount = ([regex]::Matches($content, [regex]::Escape('ersc.dll'))).Count
Write-Output "source=$source"
Write-Output "destination=$($item.FullName)"
Write-Output "size=$($item.Length)"
Write-Output "sha256=$($hash.Hash)"
Write-Output "profile=$Profile"
Write-Output "profile_entry_count=$count"
Write-Output "profile_seamless_entry_count=$seamlessCount"
Write-Output "seamless_coop_dll=$($seamlessItem.FullName)"
Write-Output "cleared_crash_telemetry=$($ClearCrashTelemetry.IsPresent)"
Write-Output "copied=$(-not $NoCopy.IsPresent)"
