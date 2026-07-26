using System.Runtime.Loader;

if (args.Length < 3 || args.Length > 4 || args[0] is "--help" or "-h")
{
    Console.Error.WriteLine("usage: route_a_mushroom_hide_armor_regulation <input-regulation.bin> <output-regulation.bin> <smithbox-dir> [summary.txt]");
    return 2;
}

var inputRegulation = Path.GetFullPath(args[0]);
var outputRegulation = Path.GetFullPath(args[1]);
var smithboxDir = Path.GetFullPath(args[2]);
var summaryPath = args.Length >= 4 ? Path.GetFullPath(args[3]) : null;

AssemblyLoadContext.Default.Resolving += (context, assemblyName) =>
{
    if (assemblyName.Name is null)
    {
        return null;
    }

    var candidate = Path.Combine(smithboxDir, assemblyName.Name + ".dll");
    return File.Exists(candidate) ? context.LoadFromAssemblyPath(candidate) : null;
};

var andreFormats = Path.Combine(smithboxDir, "Andre.Formats.dll");
var andreSoulsformats = Path.Combine(smithboxDir, "Andre.SoulsFormats.dll");
var paramdefPath = Path.Combine(smithboxDir, "Assets", "PARAM", "ER", "Defs", "EquipParamProtector.xml");
RequireFile(inputRegulation, "input regulation");
RequireFile(andreFormats, "Smithbox Andre.Formats.dll");
RequireFile(andreSoulsformats, "Smithbox Andre.SoulsFormats.dll");
RequireFile(paramdefPath, "ER EquipParamProtector paramdef");
Directory.CreateDirectory(Path.GetDirectoryName(outputRegulation) ?? ".");
if (summaryPath is not null)
{
    Directory.CreateDirectory(Path.GetDirectoryName(summaryPath) ?? ".");
}

using var binder = SoulsFormats.SFUtil.DecryptERRegulation(File.ReadAllBytes(inputRegulation));
var file = binder.Files.FirstOrDefault(entry =>
    Path.GetFileNameWithoutExtension(entry.Name).Equals(
        "EquipParamProtector", StringComparison.OrdinalIgnoreCase));
if (file is null)
{
    throw new InvalidOperationException("EquipParamProtector was not found in regulation.bin");
}

var param = SoulsFormats.PARAM.ReadIgnoreCompression(file.Bytes);
param.ApplyParamdef(SoulsFormats.PARAMDEF.XmlDeserialize(paramdefPath, true));
var fields = param.AppliedParamdef.Fields;
var equipModelId = FieldIndex(fields, "equipModelId");
var equipModelCategory = FieldIndex(fields, "equipModelCategory");
var equipModelGender = FieldIndex(fields, "equipModelGender");
var visualMaskFields = fields
    .Select((field, index) => new VisualMaskField(index, field.InternalName))
    .Where(field => field.InternalName.StartsWith("invisibleFlag", StringComparison.OrdinalIgnoreCase)
        || field.InternalName.Equals("useFaceScale", StringComparison.OrdinalIgnoreCase))
    .ToList();
var slotDefs = new[]
{
    new SlotDef("headEquip", 5, "head"),
    new SlotDef("bodyEquip", 2, "body"),
    new SlotDef("armEquip", 1, "arms"),
    new SlotDef("legEquip", 6, "legs"),
};

var changedRows = new SortedSet<int>();
var changedVisualMaskRows = new SortedSet<int>();
var changedVisualMaskFields = 0;
var alreadyHiddenRows = 0;
var touchedSlots = slotDefs.ToDictionary(slot => slot.Name, _ => 0);
var eligibleRows = 0;
foreach (var row in param.Rows)
{
    var rowHasEligibleSlot = false;
    foreach (var slot in slotDefs)
    {
        var slotFlag = Convert.ToByte(row.Cells[FieldIndex(fields, slot.FlagField)].Value);
        if (slotFlag == 0)
        {
            continue;
        }

        rowHasEligibleSlot = true;
        eligibleRows += 1;
        var currentModel = Convert.ToUInt16(row.Cells[equipModelId].Value);
        var currentCategory = Convert.ToByte(row.Cells[equipModelCategory].Value);
        var currentGender = Convert.ToByte(row.Cells[equipModelGender].Value);
        var alreadyHidden = currentModel == 0
            && currentCategory == slot.DefaultCategory
            && currentGender == 3;
        if (alreadyHidden)
        {
            alreadyHiddenRows += 1;
            continue;
        }

        row.Cells[equipModelId].Value = (ushort)0;
        row.Cells[equipModelCategory].Value = (byte)slot.DefaultCategory;
        row.Cells[equipModelGender].Value = (byte)3;
        changedRows.Add(row.ID);
        touchedSlots[slot.Name] += 1;
    }

    if (!rowHasEligibleSlot)
    {
        continue;
    }

    foreach (var maskField in visualMaskFields)
    {
        if (Convert.ToByte(row.Cells[maskField.Index].Value) == 0)
        {
            continue;
        }

        row.Cells[maskField.Index].Value = (byte)0;
        changedVisualMaskRows.Add(row.ID);
        changedVisualMaskFields += 1;
    }
}

file.Bytes = param.Write();
SoulsFormats.SFUtil.EncryptERRegulation(
    outputRegulation, binder, SoulsFormats.DCX.Type.DCX_KRAK);

var verification = VerifyOutput(outputRegulation, paramdefPath, slotDefs, visualMaskFields);
var lines = new List<string>
{
    "Route A mushroom armor-hide regulation summary",
    $"input_regulation={inputRegulation}",
    $"output_regulation={outputRegulation}",
    $"equip_param_rows={param.Rows.Count}",
    $"eligible_slot_rows={eligibleRows}",
    $"already_hidden_slot_rows={alreadyHiddenRows}",
    $"changed_rows={changedRows.Count}",
    $"changed_head_rows={touchedSlots["head"]}",
    $"changed_body_rows={touchedSlots["body"]}",
    $"changed_arm_rows={touchedSlots["arms"]}",
    $"changed_leg_rows={touchedSlots["legs"]}",
    $"changed_visual_mask_rows={changedVisualMaskRows.Count}",
    $"changed_visual_mask_fields={changedVisualMaskFields}",
    $"visual_mask_fields={visualMaskFields.Count}",
    $"verified_hidden_slot_rows={verification.HiddenSlotRows}",
    $"verified_bad_slot_rows={verification.BadSlotRows}",
    $"verified_visual_mask_rows={verification.VisualMaskRows}",
    $"verified_nonzero_visual_mask_fields={verification.NonZeroVisualMaskFields}",
    "runtime_status=not launched; regulation-only visual equipment-model and hide-mask override", 
};
if (summaryPath is not null)
{
    File.WriteAllLines(summaryPath, lines);
}
foreach (var line in lines)
{
    Console.WriteLine(line);
}
return verification.BadSlotRows == 0 && verification.NonZeroVisualMaskFields == 0 ? 0 : 1;

static void RequireFile(string path, string description)
{
    if (!File.Exists(path))
    {
        throw new FileNotFoundException($"missing {description}: {path}", path);
    }
}

static int FieldIndex(List<SoulsFormats.PARAMDEF.Field> fields, string internalName)
{
    var index = fields.FindIndex(field => field.InternalName == internalName);
    if (index < 0)
    {
        throw new InvalidOperationException($"missing EquipParamProtector field: {internalName}");
    }

    return index;
}

static VerificationReport VerifyOutput(
    string outputRegulation,
    string paramdefPath,
    IReadOnlyList<SlotDef> slotDefs,
    IReadOnlyList<VisualMaskField> visualMaskFields)
{
    using var binder = SoulsFormats.SFUtil.DecryptERRegulation(File.ReadAllBytes(outputRegulation));
    var file = binder.Files.First(entry =>
        Path.GetFileNameWithoutExtension(entry.Name).Equals(
            "EquipParamProtector", StringComparison.OrdinalIgnoreCase));
    var param = SoulsFormats.PARAM.ReadIgnoreCompression(file.Bytes);
    param.ApplyParamdef(SoulsFormats.PARAMDEF.XmlDeserialize(paramdefPath, true));
    var fields = param.AppliedParamdef.Fields;
    var equipModelId = FieldIndex(fields, "equipModelId");
    var equipModelCategory = FieldIndex(fields, "equipModelCategory");
    var equipModelGender = FieldIndex(fields, "equipModelGender");
    var hidden = 0;
    var bad = 0;
    var visualMaskRows = 0;
    var nonZeroVisualMaskFields = 0;
    foreach (var row in param.Rows)
    {
        var rowHasEligibleSlot = false;
        foreach (var slot in slotDefs)
        {
            var slotFlag = Convert.ToByte(row.Cells[FieldIndex(fields, slot.FlagField)].Value);
            if (slotFlag == 0)
            {
                continue;
            }

            rowHasEligibleSlot = true;
            var model = Convert.ToUInt16(row.Cells[equipModelId].Value);
            var category = Convert.ToByte(row.Cells[equipModelCategory].Value);
            var gender = Convert.ToByte(row.Cells[equipModelGender].Value);
            if (model == 0 && category == slot.DefaultCategory && gender == 3)
            {
                hidden += 1;
            }
            else
            {
                bad += 1;
            }
        }

        if (!rowHasEligibleSlot)
        {
            continue;
        }

        visualMaskRows += 1;
        foreach (var maskField in visualMaskFields)
        {
            if (Convert.ToByte(row.Cells[maskField.Index].Value) != 0)
            {
                nonZeroVisualMaskFields += 1;
            }
        }
    }

    return new VerificationReport(hidden, bad, visualMaskRows, nonZeroVisualMaskFields);
}

readonly record struct SlotDef(string FlagField, byte DefaultCategory, string Name);
readonly record struct VisualMaskField(int Index, string InternalName);
readonly record struct VerificationReport(
    int HiddenSlotRows,
    int BadSlotRows,
    int VisualMaskRows,
    int NonZeroVisualMaskFields);
