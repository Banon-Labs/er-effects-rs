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
    const int ModeArgIndex = 0;
    const int RegulationArgIndex = 1;
    const int ParamNameArgIndex = 2;
    const int RowIdArgStartIndex = 3;
    const int RequiredArgCount = 4;
    const int SuccessExitCode = 0;
    const int FailureExitCode = 1;
    const int UsageExitCode = 2;

    if (args.Length < RequiredArgCount || args[ModeArgIndex] != ParamRowsMode)
    {
        Console.Error.WriteLine("usage: soulsformats-bridge param-rows <regulation.bin> <param-name> <row-id> [row-id...]");
        return UsageExitCode;
    }

    var regulationPath = args[RegulationArgIndex];
    var paramName = args[ParamNameArgIndex];
    var requestedIds = args[RowIdArgStartIndex..].Select(int.Parse).ToArray();

    try
    {
        var data = File.ReadAllBytes(regulationPath);
        using var binder = SoulsFormats.SFUtil.DecryptERRegulation(data);

        // Regulation binder entry names can carry Windows-style paths (e.g.
        // "N:\\...\\SpEffectParam.param"). Path.GetFileNameWithoutExtension only
        // treats the *current platform's* separator as a directory boundary, so on
        // Linux a '\\' is kept verbatim and the stem never matches. Normalize to
        // '/' first so the bare param name is recovered on every OS.
        static string ParamStem(string name) =>
            Path.GetFileNameWithoutExtension(name.Replace('\\', '/'));

        var binderFile = binder.Files.FirstOrDefault(file =>
            ParamStem(file.Name).Equals(paramName, StringComparison.OrdinalIgnoreCase));

        if (binderFile is null)
        {
            var available = binder.Files
                .Select(file => ParamStem(file.Name))
                .Where(name => name.Length > 0)
                .OrderBy(name => name, StringComparer.OrdinalIgnoreCase)
                .ToList();
            Console.Error.WriteLine(
                $"Param not found: {paramName}. {available.Count} entries available; first 20: "
                + string.Join(", ", available.Take(20)));
            return FailureExitCode;
        }

        var param = Andre.Formats.Param.ReadIgnoreCompression(binderFile!.Bytes);
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
