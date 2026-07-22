param(
  [string]$ProfilesDir = "X:\Documents\me3 profiles",
  [string]$ProfileName = "er_net_effects_only.me3",
  [string]$DllPath = "X:\Documents\me3 profiles\er_net_effects_dll.dll"
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $ProfilesDir)) {
  throw "missing ME3 profiles directory: $ProfilesDir"
}
if (-not (Test-Path -LiteralPath $DllPath)) {
  throw "missing er-net-effects DLL: $DllPath"
}

$profilePath = Join-Path $ProfilesDir $ProfileName
$content = @"
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = '$DllPath'
"@
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($profilePath, $content, $utf8NoBom)

$profileText = Get-Content -LiteralPath $profilePath -Raw
$nativeCount = ([regex]::Matches($profileText, '^\s*\[\[natives\]\]', [System.Text.RegularExpressions.RegexOptions]::Multiline)).Count
$packageCount = ([regex]::Matches($profileText, '^\s*\[\[packages\]\]', [System.Text.RegularExpressions.RegexOptions]::Multiline)).Count
$dllMentionCount = ([regex]::Matches($profileText, [regex]::Escape('er_net_effects_dll.dll'))).Count
if ($nativeCount -ne 1) { throw "expected exactly one native entry, got $nativeCount" }
if ($packageCount -ne 0) { throw "expected zero package entries, got $packageCount" }
if ($dllMentionCount -ne 1) { throw "expected exactly one er_net_effects_dll mention, got $dllMentionCount" }
if ($profileText -match 'ersc\.dll|er_inventory_sort|er-cutscene-replacer|mushroom_man|er_effects_rs\.dll') {
  throw "isolated profile contains an unrelated native/package entry"
}

$dllItem = Get-Item -LiteralPath $DllPath
$dllHash = Get-FileHash -Algorithm SHA256 -LiteralPath $DllPath
$profileItem = Get-Item -LiteralPath $profilePath
Write-Output "profile=$($profileItem.FullName)"
Write-Output "profile_size=$($profileItem.Length)"
Write-Output "native_count=$nativeCount"
Write-Output "package_count=$packageCount"
Write-Output "dll=$($dllItem.FullName)"
Write-Output "dll_size=$($dllItem.Length)"
Write-Output "dll_sha256=$($dllHash.Hash)"
