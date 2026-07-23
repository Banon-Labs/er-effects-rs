param(
  [string]$GameDir = "C:\SteamLibrary\steamapps\common\ELDEN RING\Game",
  [string]$MasterCatalogPath = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$effectsJson = Join-Path $repoRoot "data\effects.json"
if (-not (Test-Path -LiteralPath $effectsJson)) {
  throw "missing bundled effects data: $effectsJson"
}
if (-not (Test-Path -LiteralPath $GameDir)) {
  throw "missing game directory: $GameDir"
}

$effects = Get-Content -LiteralPath $effectsJson -Raw | ConvertFrom-Json
$calls = @($effects.calls)
if ($calls.Count -eq 0) {
  throw "bundled effects data has no calls: $effectsJson"
}

$catalogDir = Join-Path $GameDir "er-net-effect-catalogs"
New-Item -ItemType Directory -Path $catalogDir -Force | Out-Null

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
function Write-Utf8Json($Path, $Object, [int]$Depth = 32) {
  $json = ConvertTo-Json -InputObject $Object -Depth $Depth
  [System.IO.File]::WriteAllText($Path, $json + "`n", $utf8NoBom)
}

function Write-Utf8Text($Path, [string]$Text) {
  [System.IO.File]::WriteAllText($Path, $Text, $utf8NoBom)
}

function Get-EffectComment($EffectById, [int]$Id) {
  if (-not $EffectById.ContainsKey($Id)) { return "" }
  $effect = $EffectById[$Id]
  $label = [string]$effect.name
  if ([string]::IsNullOrWhiteSpace($label)) { $label = [string]$effect.curated_name }
  if ([string]::IsNullOrWhiteSpace($label)) { $label = [string]$effect.row_name }
  $label = (($label -replace "\s+", " ") -replace "\*/", "* /").Trim()
  return $label
}

function ConvertTo-JsoncCatalog($Ids, $EffectById) {
  $idsArray = @($Ids)
  if ($idsArray.Count -eq 0) { return "[]`n" }
  $lines = New-Object System.Collections.Generic.List[string]
  $lines.Add("[")
  for ($index = 0; $index -lt $idsArray.Count; $index++) {
    $id = [int]$idsArray[$index]
    $comma = if ($index -lt ($idsArray.Count - 1)) { "," } else { "" }
    $comment = Get-EffectComment $EffectById $id
    $suffix = if ([string]::IsNullOrWhiteSpace($comment)) { "" } else { " // $comment" }
    $lines.Add("  $id$comma$suffix")
  }
  $lines.Add("]")
  return [string]::Join("`n", $lines) + "`n"
}

function Write-Utf8JsoncCatalog($Path, $Ids, $EffectById) {
  Write-Utf8Text -Path $Path -Text (ConvertTo-JsoncCatalog $Ids $EffectById)
}

function New-BundledFallbackMaster($Calls) {
  $masterEffects = foreach ($call in $Calls) {
    [ordered]@{
      id = [int]$call.id
      name = [string]$call.name
      row_name = $null
      community_name = $null
      curated_name = $null
      vfx = @()
      tags = @("bundled")
      fields = [ordered]@{}
    }
  }
  [ordered]@{
    schema_version = 1
    kind = "sp_effect_master_catalog"
    source = [ordered]@{
      param = "SpEffectParam"
      binder_version = ""
      row_count = $Calls.Count
      regulation_file = "data/effects.json"
      paramdef_file = ""
      names_file = ""
    }
    field_index = [ordered]@{}
    effects = @($masterEffects)
  }
}

function Get-EffectTags($Effect) {
  if ($null -eq $Effect.tags) { return @() }
  return @($Effect.tags | ForEach-Object { [string]$_ })
}

function Get-EffectFieldNames($Effect) {
  if ($null -eq $Effect.fields) { return @() }
  return @($Effect.fields.PSObject.Properties.Name)
}

function Test-EffectTag($Effect, [string]$Tag) {
  return (Get-EffectTags $Effect) -contains $Tag
}

function Test-EffectAiTag($Effect) {
  foreach ($tag in (Get-EffectTags $Effect)) {
    if ($tag.StartsWith("ai.")) { return $true }
  }
  return $false
}

function Test-EffectFieldMatch($Effect, [string]$Pattern) {
  foreach ($name in (Get-EffectFieldNames $Effect)) {
    if ($name -match $Pattern) { return $true }
  }
  return $false
}

function Test-EffectVfx($Effect) {
  if (Test-EffectTag $Effect "presentation.vfx") { return $true }
  foreach ($name in (Get-EffectFieldNames $Effect)) {
    if ($name.StartsWith("vfxId")) { return $true }
  }
  return $false
}

function Test-EffectAppearAiSound($Effect) {
  return (Get-EffectFieldNames $Effect) -contains "AppearAiSoundId"
}

function Test-EffectAudio($Effect) {
  return (Test-EffectTag $Effect "presentation.audio") -or (Test-EffectAppearAiSound $Effect)
}

$statFieldPattern = "changeHp|maxHp|hpRecover|isHpBurn|destinedDeathHp|conditionHp|changeMp|maxMp|magicConsumption|miracleConsumption|artsConsumption|goodsConsumption|shamanConsumption|changeStamina|maxStamina|staminaRecover|consumeStamina|guardStamina|addStrength|changeStrength|bAdjustStrength|addDexterity|dexterityCancel|addMagic|changeMagic|bAdjustMagic|addFaith|bAdjustFaith|addLuck|equipWeight|allItemWeight"
$recoveryFieldPattern = "hpRecoverRate|staminaRecoverChangeSpeed|changeMp(Point|Rate)|maxMpRate|recoverArtsPoint_"
$weaponFieldPattern = "^wepParamChange$|weapon"

function Test-EffectStat($Effect) {
  return (Test-EffectTag $Effect "stat.hp") -or (Test-EffectTag $Effect "stat.fp") -or (Test-EffectTag $Effect "stat.stamina") -or (Test-EffectFieldMatch $Effect $script:statFieldPattern)
}

function Test-EffectCombat($Effect) {
  return (Test-EffectTag $Effect "combat.damage") -or (Test-EffectTag $Effect "combat.defense")
}

function Test-EffectMovement($Effect) {
  return Test-EffectTag $Effect "movement_or_timing"
}

function Test-EffectWeapon($Effect) {
  return Test-EffectFieldMatch $Effect $script:weaponFieldPattern
}

function Select-EffectIds($Effects, [scriptblock]$Predicate) {
  @($Effects | Where-Object { & $Predicate $_ } | ForEach-Object { [int]$_.id } | Sort-Object -Unique)
}

$richMaster = Join-Path $repoRoot "target\er-net-effect-master-catalog-rich.json"
if ([string]::IsNullOrWhiteSpace($MasterCatalogPath) -and (Test-Path -LiteralPath $richMaster)) {
  $MasterCatalogPath = $richMaster
}

if (-not [string]::IsNullOrWhiteSpace($MasterCatalogPath)) {
  if (-not (Test-Path -LiteralPath $MasterCatalogPath)) {
    throw "missing requested master catalog: $MasterCatalogPath"
  }
  $master = Get-Content -LiteralPath $MasterCatalogPath -Raw | ConvertFrom-Json
  $masterSource = $MasterCatalogPath
} else {
  $master = New-BundledFallbackMaster $calls
  $masterSource = "data/effects.json fallback"
}

$masterPath = Join-Path $GameDir "er-net-effect-master-catalog.json"
Write-Utf8Json -Path $masterPath -Object $master -Depth 64

$masterEffects = @($master.effects)
$effectById = @{}
foreach ($effect in $masterEffects) { $effectById[[int]$effect.id] = $effect }
$allMasterIds = @($masterEffects | ForEach-Object { [int]$_.id } | Sort-Object -Unique)
$allBundledIds = @($calls | ForEach-Object { [int]$_.id } | Sort-Object -Unique)
$bundledIdLookup = @{}
foreach ($id in $allBundledIds) { $bundledIdLookup[$id] = $true }
$namedIds = @(
  $masterEffects |
    Where-Object {
      $name = [string]$_.name
      -not [string]::IsNullOrWhiteSpace($name) -and -not $name.StartsWith("SpEffect $($_.id) (") -and $bundledIdLookup.ContainsKey([int]$_.id)
    } |
    ForEach-Object { [int]$_.id } |
    Sort-Object -Unique
)
$networkTestIds = @($allMasterIds | Where-Object { $_ -eq 8355 })
if ($networkTestIds.Count -eq 0) { $networkTestIds = @(8355) }

$catalogs = [ordered]@{
  "ai-perception-targeting.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectAiTag $effect }
  "all-bundled-effects.jsonc" = $allBundledIds
  "all-sp-effects.jsonc" = $allMasterIds
  "damage-buffs.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectTag $effect "combat.damage" }
  "defense-buffs.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectTag $effect "combat.defense" }
  "fp-recovery.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectFieldMatch $effect $script:recoveryFieldPattern }
  "hp-fp-stats.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectStat $effect }
  "hp-fp-stats-only.jsonc" = Select-EffectIds $masterEffects { param($effect) (Test-EffectStat $effect) -and -not (Test-EffectVfx $effect) -and -not (Test-EffectAudio $effect) -and -not (Test-EffectCombat $effect) -and -not (Test-EffectMovement $effect) -and -not (Test-EffectWeapon $effect) -and -not (Test-EffectAiTag $effect) }
  "movement-and-timing.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectMovement $effect }
  "named-effects.jsonc" = $namedIds
  "network-test.jsonc" = $networkTestIds
  "regen-and-recovery.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectFieldMatch $effect $script:recoveryFieldPattern }
  "sound-effects.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectAudio $effect }
  "sounds-only.jsonc" = Select-EffectIds $masterEffects { param($effect) (Test-EffectAppearAiSound $effect) -and -not (Test-EffectVfx $effect) -and -not (Test-EffectStat $effect) -and -not (Test-EffectCombat $effect) -and -not (Test-EffectMovement $effect) -and -not (Test-EffectWeapon $effect) }
  "visual-effects.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectVfx $effect }
  "visuals-only.jsonc" = Select-EffectIds $masterEffects { param($effect) (Test-EffectVfx $effect) -and -not (Test-EffectAudio $effect) -and -not (Test-EffectStat $effect) -and -not (Test-EffectCombat $effect) -and -not (Test-EffectMovement $effect) -and -not (Test-EffectWeapon $effect) -and -not (Test-EffectAiTag $effect) }
  "weapon-buffs.jsonc" = Select-EffectIds $masterEffects { param($effect) Test-EffectWeapon $effect }
}

foreach ($pattern in @("*.json", "*.jsonc")) {
  Get-ChildItem -LiteralPath $catalogDir -Filter $pattern -File | Remove-Item -Force
}
$catalogCountByName = @{}
foreach ($entry in $catalogs.GetEnumerator()) {
  $ids = @($entry.Value)
  $catalogCountByName[$entry.Key] = $ids.Count
  Write-Utf8JsoncCatalog -Path (Join-Path $catalogDir $entry.Key) -Ids $ids -EffectById $effectById
}

$installed = Get-ChildItem -LiteralPath $catalogDir -Filter *.jsonc -File | Sort-Object Name
Write-Output "game_dir=$GameDir"
Write-Output "master_catalog=$masterPath"
Write-Output "master_source=$masterSource"
Write-Output "catalog_dir=$catalogDir"
Write-Output "master_effect_count=$($masterEffects.Count)"
Write-Output "bundled_call_count=$($calls.Count)"
foreach ($file in $installed) {
  $idCount = $catalogCountByName[$file.Name]
  Write-Output "catalog_file=$($file.FullName) count=$idCount size=$($file.Length)"
}
