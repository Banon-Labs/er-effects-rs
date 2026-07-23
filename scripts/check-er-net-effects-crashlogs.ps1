param(
  [double]$SinceHours = 2,
  [string]$GameDir = "C:\SteamLibrary\steamapps\common\ELDEN RING\Game",
  [int]$MaxDumps = 8,
  [switch]$SkipWer
)

$ErrorActionPreference = "SilentlyContinue"
$now = Get-Date
$since = $now.AddHours(-1 * [math]::Abs($SinceHours))
$crashDir = Join-Path $GameDir "SeamlessCoop\crashdumps\reports"

Write-Output "now=$($now.ToString('o')) since=$($since.ToString('o')) game_dir=$GameDir"
Write-Output "== process state =="
$procs = Get-Process |
  Where-Object { $_.ProcessName -match 'elden|start_protected|me3|modengine' } |
  Select-Object Id, ProcessName, MainWindowTitle, MainWindowHandle, StartTime
if ($procs) {
  $procs | Format-List | Out-String | Write-Output
} else {
  Write-Output "no matching live game/modengine processes"
}

Write-Output "== newest Seamless crash dumps =="
if (Test-Path -LiteralPath $crashDir) {
  Get-ChildItem -LiteralPath $crashDir -Filter *.dmp -File |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First $MaxDumps |
    ForEach-Object {
      Write-Output "$($_.LastWriteTime.ToString('o')) size=$($_.Length) $($_.FullName)"
    }
} else {
  Write-Output "missing crash dir $crashDir"
}

Write-Output "== er-net-effects crash telemetry artifacts =="
foreach ($name in @(
  "er-net-effects-crash-telemetry-latest.txt",
  "er-net-effects-crash-telemetry.log",
  "er-net-effects-breadcrumb-latest.txt",
  "er-net-effects-telemetry.json",
  "er-net-effects.log"
)) {
  $path = Join-Path $GameDir $name
  if (Test-Path -LiteralPath $path) {
    $item = Get-Item -LiteralPath $path
    Write-Output "--- file=$path size=$($item.Length) mtime=$($item.LastWriteTime.ToString('o')) ---"
    if ($item.Length -lt 20000) {
      Get-Content -LiteralPath $path -Raw | Write-Output
    } else {
      Get-Content -LiteralPath $path -Tail 160 | Write-Output
    }
  } else {
    Write-Output "missing $path"
  }
}

if ($SkipWer) {
  exit 0
}

Write-Output "== Application crash/hang/WER events =="
$events = Get-WinEvent -FilterHashtable @{LogName = 'Application'; StartTime = $since; Level = 1, 2, 3} |
  Where-Object {
    ($_.ProviderName -match 'Application Error|Windows Error Reporting|Application Hang|\.NET Runtime') -or
    ($_.Message -match 'elden|er_net_effects|er-effects|modengine|me3|d3d12|dxgi|ersc')
  } |
  Where-Object { $_.Message -match 'elden|er_net_effects|er-effects|modengine|me3|d3d12|dxgi|vkd3d|ersc|exception|fault|crash|hang' } |
  Sort-Object TimeCreated -Descending |
  Select-Object -First 20
if ($events) {
  foreach ($event in $events) {
    Write-Output "--- event time=$($event.TimeCreated.ToString('o')) provider=$($event.ProviderName) id=$($event.Id) level=$($event.LevelDisplayName) ---"
    ($event.Message -split "`r?`n" | Select-Object -First 22) -join "`n" | Write-Output
  }
} else {
  Write-Output "no matching Application Error/WER events"
}

Write-Output "== WER Report.wer files matching game/mod DLLs =="
$werRoots = @(
  "$env:LOCALAPPDATA\Microsoft\Windows\WER\ReportArchive",
  "$env:LOCALAPPDATA\Microsoft\Windows\WER\ReportQueue",
  "$env:ProgramData\Microsoft\Windows\WER\ReportArchive",
  "$env:ProgramData\Microsoft\Windows\WER\ReportQueue"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
$reports = foreach ($root in $werRoots) {
  Get-ChildItem -LiteralPath $root -Recurse -Filter Report.wer |
    Where-Object { $_.LastWriteTime -ge $since }
}
$matched = @()
foreach ($report in ($reports | Sort-Object LastWriteTime -Descending)) {
  $text = Get-Content -LiteralPath $report.FullName -Raw
  if ($text -match 'elden|er_net_effects|er-effects|modengine|me3|d3d12|dxgi|ersc') {
    $matched += $report
    Write-Output "--- WER mtime=$($report.LastWriteTime.ToString('o')) path=$($report.FullName) ---"
    $keys = 'AppName=|FriendlyEventName=|EventType=|Sig\[0\]=|Sig\[1\]=|Sig\[2\]=|Sig\[3\]=|Sig\[4\]=|Sig\[5\]=|Sig\[6\]=|Sig\[7\]=|DynamicSig\[1\]=|DynamicSig\[2\]=|LoadedModule\[|FaultingModule|ExceptionCode|CabGuid='
    ($text -split "`r?`n" | Where-Object { $_ -match $keys } | Select-Object -First 90) -join "`n" | Write-Output
  }
}
if (-not $matched) {
  Write-Output "no matching WER Report.wer files"
}
