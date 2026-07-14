#!/usr/bin/env python3
"""Generate the SpEffect master catalog from a local Elden Ring regulation.bin.

The master catalog is rich metadata keyed by SpEffectParam ID. User-facing
selector catalogs should stay separate and contain only lists of IDs.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import textwrap
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_REGULATION = Path(
    "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/regulation.bin"
)
DEFAULT_PARAMDEF = Path(
    "/home/banon/projects/WitchyBND/WitchyBND/Assets/Paramdex/ER/Defs/SpEffect.xml"
)
DEFAULT_NAMES = Path(
    "/home/banon/projects/WitchyBND/WitchyBND/Assets/Paramdex/ER/Names/SpEffectParam.txt"
)
DEFAULT_OUTPUT = REPO_ROOT / "data" / "effect-master-catalog.json"
DEFAULT_SMITHBOX_BINARY_DIR = Path("/home/banon/.local/share/smithbox/app")


PROGRAM = r'''
using System.Globalization;
using System.Runtime.Loader;
using System.Text.Json;
using System.Text.Json.Serialization;
using System.Text.RegularExpressions;
using System.Xml.Linq;

var smithboxBinaryDir = Environment.GetEnvironmentVariable("SMITHBOX_BINARY_DIR");
if (!string.IsNullOrEmpty(smithboxBinaryDir))
{
    AssemblyLoadContext.Default.Resolving += (context, assemblyName) =>
    {
        if (assemblyName.Name is null) return null;
        var candidate = Path.Combine(smithboxBinaryDir, assemblyName.Name + ".dll");
        return File.Exists(candidate) ? context.LoadFromAssemblyPath(candidate) : null;
    };
}

if (args.Length != 5)
{
    Console.Error.WriteLine("usage: generator <regulation.bin> <paramdef.xml> <names.txt> <effects.json> <output.json>");
    Environment.Exit(2);
}

var regulationPath = args[0];
var paramdefPath = args[1];
var namesPath = args[2];
var effectsJsonPath = args[3];
var outputPath = args[4];

var fieldDefs = LoadFieldDefs(paramdefPath);
var communityNames = LoadNames(namesPath);
var curatedNames = LoadCuratedNames(effectsJsonPath);

var data = File.ReadAllBytes(regulationPath);
using var binder = SoulsFormats.SFUtil.DecryptERRegulation(data);
var binderFile = binder.Files.First(file =>
{
    var normalizedName = file.Name.Replace('\\', '/');
    var stem = Path.GetFileNameWithoutExtension(normalizedName);
    var doubleStem = Path.GetFileNameWithoutExtension(stem);
    return string.Equals(stem, "SpEffectParam", StringComparison.OrdinalIgnoreCase)
        || string.Equals(doubleStem, "SpEffectParam", StringComparison.OrdinalIgnoreCase);
});
var param = Andre.Formats.Param.ReadIgnoreCompression(binderFile.Bytes);
var paramdef = SoulsFormats.PARAMDEF.XmlDeserialize(paramdefPath);
param.ApplyParamdef(paramdef, ulong.MaxValue, "");

var effects = new List<MasterEffect>();
foreach (var row in param.Rows.OrderBy(row => row.ID))
{
    var fields = new SortedDictionary<string, object?>();
    var tags = new SortedSet<string>();
    var vfx = new List<int>();

    foreach (var cell in row.Cells)
    {
        var name = cell.Def.InternalName;
        if (string.IsNullOrWhiteSpace(name) || name.StartsWith("pad", StringComparison.OrdinalIgnoreCase))
        {
            continue;
        }
        var value = cell.Value;
        if (name == "vfxId" || Regex.IsMatch(name, "^vfxId[1-7]$"))
        {
            if (TryInt(value, out var vfxId) && vfxId >= 0)
            {
                vfx.Add(vfxId);
            }
        }
        if (IsApplicabilityField(name))
        {
            continue;
        }
        if (!fieldDefs.TryGetValue(name, out var fieldDef))
        {
            fieldDef = new FieldDef(name, "", "0", "unknown");
        }
        if (IsDefaultValue(value, SemanticDefaultFor(name, fieldDef.DefaultValue)))
        {
            continue;
        }
        fields[name] = NormalizeValue(value);
        foreach (var tag in TagsForField(name, value))
        {
            tags.Add(tag);
        }
    }

    if (vfx.Count > 0)
    {
        tags.Add("presentation.vfx");
    }

    curatedNames.TryGetValue(row.ID, out var curatedName);
    communityNames.TryGetValue(row.ID, out var communityName);
    var rowName = row.Name ?? "";
    var displayName = FirstNonEmpty(curatedName, rowName, communityName);

    effects.Add(new MasterEffect(
        row.ID,
        displayName,
        EmptyToNull(rowName),
        EmptyToNull(communityName),
        EmptyToNull(curatedName),
        vfx.Distinct().OrderBy(id => id).ToArray(),
        tags.ToArray(),
        fields));
}

var catalog = new MasterCatalog(
    1,
    "sp_effect_master_catalog",
    new MasterCatalogSource("SpEffectParam", binder.Version.ToString(), param.Rows.Count, Path.GetFileName(regulationPath), Path.GetFileName(paramdefPath), Path.GetFileName(namesPath)),
    BuildFieldIndex(fieldDefs),
    effects);

Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(outputPath))!);
var jsonOptions = new JsonSerializerOptions
{
    WriteIndented = true,
    DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
};
File.WriteAllText(outputPath, JsonSerializer.Serialize(catalog, jsonOptions) + "\n");
Console.WriteLine($"wrote {effects.Count} SpEffect master entries to {outputPath}");
return 0;

static string FirstNonEmpty(params string?[] values) => values.FirstOrDefault(value => !string.IsNullOrWhiteSpace(value)) ?? "";
static string? EmptyToNull(string? value) => string.IsNullOrWhiteSpace(value) ? null : value;

static bool TryInt(object? value, out int parsed)
{
    return int.TryParse(Convert.ToString(value, CultureInfo.InvariantCulture), NumberStyles.Integer, CultureInfo.InvariantCulture, out parsed);
}

static object? NormalizeValue(object? value)
{
    if (value is null) return null;
    if (value is bool or byte or sbyte or short or ushort or int or uint or long or ulong or float or double or decimal) return value;
    var text = Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
    if (long.TryParse(text, NumberStyles.Integer, CultureInfo.InvariantCulture, out var integer)) return integer;
    if (double.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out var number)) return number;
    return text;
}

static string SemanticDefaultFor(string name, string parsedDefault)
{
    if (Regex.IsMatch(name, "^vowType\\d+$")) return "1";
    var defaultOneFields = new HashSet<string>
    {
        "allItemWeightChangeRate",
        "equipWeightChangeRate",
        "fallDamageRate",
        "hpRecoverRate",
        "lifeReductionRate",
        "soulRate",
        "soulStealRate",
    };
    if (defaultOneFields.Contains(name)) return "1";
    return parsedDefault;
}

static bool IsApplicabilityField(string name)
{
    return name.StartsWith("effectTarget", StringComparison.OrdinalIgnoreCase)
        || Regex.IsMatch(name, "^vowType\\d+$");
}

static bool IsDefaultValue(object? value, string defaultValue)
{
    var normalized = Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
    if (double.TryParse(normalized, NumberStyles.Float, CultureInfo.InvariantCulture, out var number)
        && double.TryParse(defaultValue, NumberStyles.Float, CultureInfo.InvariantCulture, out var defaultNumber))
    {
        return Math.Abs(number - defaultNumber) < 0.000001;
    }
    return string.Equals(normalized, defaultValue, StringComparison.OrdinalIgnoreCase);
}

static Dictionary<string, FieldDef> LoadFieldDefs(string paramdefPath)
{
    var defs = new Dictionary<string, FieldDef>();
    var doc = XDocument.Load(paramdefPath);
    foreach (var field in doc.Descendants("Field"))
    {
        var def = field.Attribute("Def")?.Value ?? "";
        var parsed = ParseDef(def);
        if (parsed is null) continue;
        var displayName = field.Element("DisplayName")?.Value ?? "";
        defs[parsed.Value.Name] = new FieldDef(parsed.Value.Name, displayName, parsed.Value.DefaultValue, parsed.Value.TypeName);
    }
    return defs;
}

static (string TypeName, string Name, string DefaultValue)? ParseDef(string def)
{
    var match = Regex.Match(def.Trim(), @"^(?<type>\S+)\s+(?<name>[A-Za-z_][A-Za-z0-9_]*)(?::\d+)?(?:\[[^\]]+\])?(?:\s*=\s*(?<default>\S+))?");
    if (!match.Success) return null;
    var type = match.Groups["type"].Value;
    var name = match.Groups["name"].Value;
    var defaultValue = match.Groups["default"].Success ? match.Groups["default"].Value : "0";
    return (type, name, defaultValue);
}

static Dictionary<int, string> LoadNames(string namesPath)
{
    var names = new Dictionary<int, string>();
    if (!File.Exists(namesPath)) return names;
    foreach (var line in File.ReadLines(namesPath))
    {
        var trimmed = line.Trim();
        if (trimmed.Length == 0) continue;
        var match = Regex.Match(trimmed, @"^(?<id>-?\d+)\s*(?<name>.*)$");
        if (!match.Success) continue;
        var id = int.Parse(match.Groups["id"].Value, CultureInfo.InvariantCulture);
        var name = match.Groups["name"].Value.Trim();
        if (name.Length > 0) names[id] = name;
    }
    return names;
}

static Dictionary<int, string> LoadCuratedNames(string effectsJsonPath)
{
    if (!File.Exists(effectsJsonPath)) return new Dictionary<int, string>();
    var parsed = JsonSerializer.Deserialize<EffectsFile>(File.ReadAllText(effectsJsonPath));
    return parsed?.calls.ToDictionary(call => call.id, call => call.name) ?? new Dictionary<int, string>();
}

static string[] TagsForField(string name, object? value)
{
    var lower = name.ToLowerInvariant();
    var tags = new List<string>();
    if (lower.Contains("hp")) tags.Add("stat.hp");
    if (lower.Contains("mp") || lower.Contains("fp")) tags.Add("stat.fp");
    if (lower.Contains("stamina")) tags.Add("stat.stamina");
    if (lower.Contains("sight") || lower.Contains("hearing") || lower.Contains("aisound") || lower.Contains("aware")) tags.Add("ai.perception");
    if (lower == "cleartarget" || lower == "targetpriority" || lower.Contains("teamtype") || lower.Contains("searchednotify")) tags.Add("ai.targeting");
    if (lower.Contains("movespeed") || lower.Contains("staminaattack") || lower.Contains("motion")) tags.Add("movement_or_timing");
    if (lower.Contains("attack") || lower.Contains("damage") || lower.Contains("dmg")) tags.Add("combat.damage");
    if (lower.Contains("defence") || lower.Contains("diffence") || lower.Contains("cutrate")) tags.Add("combat.defense");
    if (lower.StartsWith("vfx")) tags.Add("presentation.vfx");
    if (lower.Contains("sfx") || lower.Contains("sound")) tags.Add("presentation.audio");
    if (lower.Contains("duration") || lower.Contains("endurance") || lower.Contains("interval")) tags.Add("lifetime");

    var normalized = Convert.ToString(value, CultureInfo.InvariantCulture) ?? "";
    if ((name == "sightSearchEnemyRate" || name == "hearingSearchEnemyRate") && normalized == "0") tags.Add("ai.perception.zero");
    if (name == "clearTarget" && normalized == "1") tags.Add("ai.targeting.clear_target");
    if (name == "targetPriority" && double.TryParse(normalized, NumberStyles.Float, CultureInfo.InvariantCulture, out var priority) && priority < 0) tags.Add("ai.targeting.lower_priority");
    return tags.ToArray();
}

static Dictionary<string, FieldIndexEntry> BuildFieldIndex(Dictionary<string, FieldDef> fieldDefs)
{
    return fieldDefs.OrderBy(pair => pair.Key).ToDictionary(
        pair => pair.Key,
        pair => new FieldIndexEntry(pair.Value.TypeName, EmptyToNull(pair.Value.DisplayName), TagsForField(pair.Key, null)));
}

record EffectsFile(List<EffectSpec> calls);
record EffectSpec(string kind, int id, string name, bool enabled);
record FieldDef(string Name, string DisplayName, string DefaultValue, string TypeName);
record MasterCatalog(int schema_version, string kind, MasterCatalogSource source, Dictionary<string, FieldIndexEntry> field_index, List<MasterEffect> effects);
record MasterCatalogSource(string param, string binder_version, int row_count, string regulation_file, string paramdef_file, string names_file);
record FieldIndexEntry(string type, string? display_name, string[] tags);
record MasterEffect(int id, string name, string? row_name, string? community_name, string? curated_name, int[] vfx, string[] tags, SortedDictionary<string, object?> fields);
'''


CSPROJ = r'''
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net9.0</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
    <LangVersion>12</LangVersion>
    <NoWarn>MSB3277</NoWarn>
  </PropertyGroup>
  <ItemGroup>
    <Reference Include="Andre.Formats">
      <HintPath>{smithbox}/Andre.Formats.dll</HintPath>
    </Reference>
    <Reference Include="Andre.SoulsFormats">
      <HintPath>{smithbox}/Andre.SoulsFormats.dll</HintPath>
    </Reference>
  </ItemGroup>
</Project>
'''


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--regulation", type=Path, default=DEFAULT_REGULATION)
    parser.add_argument("--paramdef", type=Path, default=DEFAULT_PARAMDEF)
    parser.add_argument("--names", type=Path, default=DEFAULT_NAMES)
    parser.add_argument("--effects", type=Path, default=REPO_ROOT / "data" / "effects.json")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument(
        "--smithbox-binary-dir",
        type=Path,
        default=Path(os.environ.get("SMITHBOX_BINARY_DIR", DEFAULT_SMITHBOX_BINARY_DIR)),
    )
    return parser.parse_args()


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"missing {label}: {path}")


def main() -> int:
    args = parse_args()
    require_file(args.regulation, "regulation.bin")
    require_file(args.paramdef, "SpEffect paramdef")
    require_file(args.effects, "effects catalog")
    require_file(args.smithbox_binary_dir / "Andre.Formats.dll", "Andre.Formats.dll")
    require_file(args.smithbox_binary_dir / "Andre.SoulsFormats.dll", "Andre.SoulsFormats.dll")

    work_dir = REPO_ROOT / "target" / "effect-master-catalog-generator"
    work_dir.mkdir(parents=True, exist_ok=True)
    (work_dir / "Program.cs").write_text(PROGRAM, encoding="utf-8")
    (work_dir / "effect-master-catalog-generator.csproj").write_text(
        CSPROJ.format(smithbox=args.smithbox_binary_dir), encoding="utf-8"
    )

    env = os.environ.copy()
    env["DOTNET_ROLL_FORWARD"] = env.get("DOTNET_ROLL_FORWARD", "Major")
    env["SMITHBOX_BINARY_DIR"] = str(args.smithbox_binary_dir)
    command = [
        "dotnet",
        "run",
        "--project",
        str(work_dir / "effect-master-catalog-generator.csproj"),
        "-v",
        "quiet",
        "--",
        str(args.regulation),
        str(args.paramdef),
        str(args.names),
        str(args.effects),
        str(args.output),
    ]
    try:
        result = subprocess.run(
            command,
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            timeout=30,
        )
    except subprocess.TimeoutExpired as error:
        print(error.stdout or "", end="")
        print(error.stderr or "", end="")
        print("effect master catalog generation timed out", file=os.sys.stderr)
        return 124
    if result.returncode != 0:
        print(result.stdout, end="")
        print(result.stderr, end="")
        return result.returncode
    print(result.stdout.strip())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
