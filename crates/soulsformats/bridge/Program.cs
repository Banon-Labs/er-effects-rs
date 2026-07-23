using System.Runtime.Loader;

const string SmithboxBinaryDirEnv = "SMITHBOX_BINARY_DIR";

// When the bridge was built against a binary Smithbox install (DLL references
// instead of a project reference), transitive Smithbox dependencies are not
// copied to the bridge output directory. Resolve them from the install
// directory instead. This must be registered before any method touching
// Andre/SoulsFormats types is JIT-compiled, which is why the actual work
// lives in the Run local function below.
var smithboxBinaryDir = Environment.GetEnvironmentVariable(SmithboxBinaryDirEnv);
if (!string.IsNullOrEmpty(smithboxBinaryDir))
{
    AssemblyLoadContext.Default.Resolving += (context, assemblyName) =>
    {
        if (assemblyName.Name is null)
        {
            return null;
        }
        var candidate = Path.Combine(smithboxBinaryDir, assemblyName.Name + ".dll");
        return File.Exists(candidate) ? context.LoadFromAssemblyPath(candidate) : null;
    };
}

return Run(args);

static int Run(string[] args)
{
    const string ParamRowsMode = "param-rows";
    const string ParamListMode = "param-list";
    const string ParamRowDumpMode = "param-row-dump";
    const string ParamVfxCandidatesMode = "param-vfx-candidates";
    const int ModeArgIndex = 0;
    const int RegulationArgIndex = 1;
    const int ParamNameArgIndex = 2;
    const int RowIdArgStartIndex = 3;
    const int RequiredArgCount = 4;
    const int SuccessExitCode = 0;
    const int FailureExitCode = 1;
    const int UsageExitCode = 2;

    if (args.Length < 2 || (args[ModeArgIndex] != ParamRowsMode && args[ModeArgIndex] != ParamListMode && args[ModeArgIndex] != ParamRowDumpMode && args[ModeArgIndex] != ParamVfxCandidatesMode))
    {
        Console.Error.WriteLine("usage: soulsformats-bridge param-list <regulation.bin> | param-vfx-candidates <regulation.bin> <param-name> | param-row-dump <regulation.bin> <param-name> <row-id> | param-rows <regulation.bin> <param-name> <row-id> [row-id...]");
        return UsageExitCode;
    }

    var regulationPath = args[RegulationArgIndex];

    try
    {
        var data = File.ReadAllBytes(regulationPath);
        using var binder = SoulsFormats.SFUtil.DecryptERRegulation(data);

        if (args[ModeArgIndex] == ParamListMode)
        {
            var methods = typeof(Andre.Formats.Param).GetMethods().Select(method => method.ToString()).OrderBy(method => method).ToArray();
            var files = binder.Files.Select(file => file.Name).OrderBy(name => name).ToArray();
            Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new
            {
                binder_version = binder.Version,
                file_count = files.Length,
                files,
                methods,
            }));
            return SuccessExitCode;
        }

        if ((args[ModeArgIndex] == ParamRowsMode || args[ModeArgIndex] == ParamRowDumpMode) && args.Length < RequiredArgCount)
        {
            Console.Error.WriteLine("usage: soulsformats-bridge param-row-dump <regulation.bin> <param-name> <row-id> | param-rows <regulation.bin> <param-name> <row-id> [row-id...]");
            return UsageExitCode;
        }
        if (args[ModeArgIndex] == ParamVfxCandidatesMode && args.Length < ParamNameArgIndex + 1)
        {
            Console.Error.WriteLine("usage: soulsformats-bridge param-vfx-candidates <regulation.bin> <param-name>");
            return UsageExitCode;
        }

        var paramName = args[ParamNameArgIndex];
        var requestedIds = args.Length > RowIdArgStartIndex ? args[RowIdArgStartIndex..].Select(int.Parse).ToArray() : Array.Empty<int>();
        var binderFile = binder.Files.FirstOrDefault(file =>
        {
            var normalizedName = file.Name.Replace('\\', '/');
            var stem = Path.GetFileNameWithoutExtension(normalizedName);
            var doubleStem = Path.GetFileNameWithoutExtension(stem);
            return string.Equals(stem, paramName, StringComparison.OrdinalIgnoreCase)
                || string.Equals(doubleStem, paramName, StringComparison.OrdinalIgnoreCase);
        });

        if (binderFile is null)
        {
            Console.Error.WriteLine($"Param not found: {paramName}");
            return FailureExitCode;
        }

        var param = Andre.Formats.Param.ReadIgnoreCompression(binderFile!.Bytes);
        var paramdefDir = Environment.GetEnvironmentVariable("PARAMDEX_ER_DEFS_DIR");
        if (!string.IsNullOrEmpty(paramdefDir))
        {
            var paramdefStem = paramName.EndsWith("Param", StringComparison.OrdinalIgnoreCase)
                ? paramName[..^"Param".Length]
                : paramName;
            var paramdefPath = Path.Combine(paramdefDir, paramdefStem + ".xml");
            if (File.Exists(paramdefPath))
            {
                var paramdef = SoulsFormats.PARAMDEF.XmlDeserialize(paramdefPath);
                param.ApplyParamdef(paramdef, ulong.MaxValue, "");
            }
        }

        if (args[ModeArgIndex] == ParamVfxCandidatesMode)
        {
            static string Cell(Andre.Formats.Param.Row row, string name) => row.Cells.Where(cell => cell.Def.InternalName == name).Select(cell => cell.Value?.ToString() ?? "").FirstOrDefault() ?? "";
            static int CellInt(Andre.Formats.Param.Row row, string name, int fallback = -1) => int.TryParse(Cell(row, name), out var value) ? value : fallback;
            static float CellFloat(Andre.Formats.Param.Row row, string name, float fallback = 0) => float.TryParse(Cell(row, name), out var value) ? value : fallback;
            var candidates = param.Rows.Select(row =>
            {
                var vfx = new[] { "vfxId", "vfxId1", "vfxId2", "vfxId3", "vfxId4", "vfxId5", "vfxId6", "vfxId7" }
                    .Select(name => CellInt(row, name))
                    .Where(value => value >= 0)
                    .ToArray();
                return new
                {
                    id = row.ID,
                    name = row.Name ?? "",
                    vfx,
                    effectEndurance = CellFloat(row, "effectEndurance"),
                    iconId = CellInt(row, "iconId"),
                    spCategory = CellInt(row, "spCategory", 0),
                    saveCategory = CellInt(row, "saveCategory"),
                    stateInfo = CellInt(row, "stateInfo", 0),
                    targetSelf = CellInt(row, "effectTargetSelf", 0),
                    targetPlayer = CellInt(row, "effectTargetPlayer", 0),
                    targetLive = CellInt(row, "effectTargetLive", 0),
                    targetGhost = CellInt(row, "effectTargetGhost", 0),
                    targetSelfTarget = CellInt(row, "effectTargetSelfTarget", 0),
                    maxHpRate = CellFloat(row, "maxHpRate", 1),
                    maxMpRate = CellFloat(row, "maxMpRate", 1),
                    maxStaminaRate = CellFloat(row, "maxStaminaRate", 1),
                    conditionHp = CellFloat(row, "conditionHp", -1),
                    replaceSpEffectId = CellInt(row, "replaceSpEffectId"),
                    cycleOccurrenceSpEffectId = CellInt(row, "cycleOccurrenceSpEffectId"),
                };
            }).Where(row => row.vfx.Length > 0).ToArray();
            Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new
            {
                param_name = paramName,
                row_count = param.Rows.Count,
                candidate_count = candidates.Length,
                candidates,
            }));
            return SuccessExitCode;
        }

        if (args[ModeArgIndex] == ParamRowDumpMode)
        {
            var requestedId = int.Parse(args[RowIdArgStartIndex]);
            var row = param.Rows.FirstOrDefault(row => row.ID == requestedId);
            if (row is null)
            {
                Console.Error.WriteLine($"Row not found: {requestedId}");
                return FailureExitCode;
            }
            var properties = row.GetType().GetProperties()
                .Where(prop => prop.GetIndexParameters().Length == 0)
                .Select(prop => new
                {
                    name = prop.Name,
                    type = prop.PropertyType.FullName,
                    value = prop.GetValue(row)?.ToString() ?? "",
                }).ToArray();
            var fields = row.GetType().GetFields().Select(field => new
            {
                name = field.Name,
                type = field.FieldType.FullName,
                value = field.GetValue(row)?.ToString() ?? "",
            }).ToArray();
            var cells = row.Cells.Select(cell => new
            {
                name = cell.Def.InternalName,
                type = cell.Def.DisplayType.ToString(),
                value = cell.Value?.ToString() ?? "",
            }).ToArray();
            Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(new
            {
                param_name = paramName,
                row_id = requestedId,
                row_type = row.GetType().FullName,
                properties,
                fields,
                cells,
            }));
            return SuccessExitCode;
        }

        var rows = new List<object>();
        var occurrenceIndexById = new Dictionary<int, int>();
        var foundIds = new HashSet<int>();
        var requestedIdSet = requestedIds.ToHashSet();

        foreach (var row in param.Rows)
        {
            if (!occurrenceIndexById.TryGetValue(row.ID, out var occurrenceIndex))
            {
                occurrenceIndex = 0;
            }
            occurrenceIndexById[row.ID] = occurrenceIndex + 1;

            if (!requestedIdSet.Contains(row.ID))
            {
                continue;
            }

            foundIds.Add(row.ID);
            rows.Add(new
            {
                id = row.ID,
                occurrence_index = occurrenceIndex,
                name = row.Name ?? "",
                found = true,
            });
        }

        foreach (var missingId in requestedIds.Where(id => !foundIds.Contains(id)))
        {
            rows.Add(new
            {
                id = missingId,
                occurrence_index = 0,
                name = "",
                found = false,
            });
        }

        var response = new
        {
            binder_version = binder.Version,
            param_name = paramName,
            row_count = param.Rows.Count,
            rows,
        };

        Console.WriteLine(System.Text.Json.JsonSerializer.Serialize(response));
        return SuccessExitCode;
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine(ex.ToString());
        return FailureExitCode;
    }
}
