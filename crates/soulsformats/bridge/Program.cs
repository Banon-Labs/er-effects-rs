using Andre.Formats;
using SoulsFormats;
using System.Text.Json;

const string ParamRowsMode = "param-rows";
const int ModeArgIndex = 0;
const int RegulationArgIndex = 1;
const int ParamNameArgIndex = 2;
const int RowIdArgStartIndex = 3;
const int RequiredArgCount = 4;
const int UsageExitCode = 2;
const int FailureExitCode = 1;

if (args.Length < RequiredArgCount || args[ModeArgIndex] != ParamRowsMode)
{
    Console.Error.WriteLine("usage: soulsformats-bridge param-rows <regulation.bin> <param-name> <row-id> [row-id...]");
    Environment.Exit(UsageExitCode);
}

var regulationPath = args[RegulationArgIndex];
var paramName = args[ParamNameArgIndex];
var requestedIds = args[RowIdArgStartIndex..].Select(int.Parse).ToArray();

try
{
    var data = File.ReadAllBytes(regulationPath);
    using var binder = SFUtil.DecryptERRegulation(data);
    var binderFile = binder.Files.FirstOrDefault(file =>
        Path.GetFileNameWithoutExtension(file.Name).Equals(paramName, StringComparison.OrdinalIgnoreCase));

    if (binderFile is null)
    {
        Console.Error.WriteLine($"Param not found: {paramName}");
        Environment.Exit(FailureExitCode);
    }

    var param = Param.ReadIgnoreCompression(binderFile!.Bytes);
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

    Console.WriteLine(JsonSerializer.Serialize(response));
}
catch (Exception ex)
{
    Console.Error.WriteLine(ex.ToString());
    Environment.Exit(FailureExitCode);
}
