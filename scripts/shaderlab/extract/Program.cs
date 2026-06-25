using System.Reflection;
using System.Runtime.Loader;
using Andre.Core;
using Andre.Formats;
using SoulsFormats;
using SoulsFormats.Util;

// Resolve Smithbox transitive assemblies from its install dir.
var sb = Environment.GetEnvironmentVariable("SMITHBOX_BINARY_DIR")
         ?? "/home/banon/.local/share/smithbox/app";
AssemblyLoadContext.Default.Resolving += (ctx, name) =>
{
    if (name.Name is null) return null;
    // Never serve framework assemblies (System.*) from the Smithbox install —
    // those must come from this app's own runtime / package refs, or versions clash.
    if (name.Name.StartsWith("System.")) return null;
    var cand = Path.Combine(sb, name.Name + ".dll");
    return File.Exists(cand) ? ctx.LoadFromAssemblyPath(cand) : null;
};

if (args.Length < 2)
{
    Console.Error.WriteLine("usage: extract <gameFolder> <logicalPath> [outDir]");
    Console.Error.WriteLine("       extract <gameFolder> --shaders [outDirRoot]   # survey/extract all shader containers");
    return 2;
}
var gameFolder = args[0];

// ER BHD5 path hash: 64-bit, hash = hash*PRIME64 + char over the normalized
// path (forward slashes, leading slash, lowercased). PRIME64 = 0x85 (133).
// Confirmed against Andre.Core.Util.BhdDictionary.ComputeHash.
static ulong Hash(string logicalPath)
{
    string n = logicalPath.Trim().Replace('\\', '/').ToLowerInvariant();
    if (!n.StartsWith('/')) n = "/" + n;
    ulong h = 0;
    foreach (char c in n) h = h * 0x85ul + c;
    return h;
}

// Build the target set: hash -> logical path.
var targets = new Dictionary<ulong, string>();
bool shadersMode = args[1] == "--shaders";
string outRoot;
if (shadersMode)
{
    outRoot = args.Length > 2 ? args[2] : null;
    var dictPath = Path.Combine(sb, "Assets", "UXM Dictionaries", "EldenRingDictionary.txt");
    foreach (var line in File.ReadLines(dictPath))
    {
        var p = line.Trim();
        if (p.Length == 0) continue;
        if (p.Contains("shader", StringComparison.OrdinalIgnoreCase) || p.EndsWith(".shaderbnd.dcx") || p.EndsWith(".shaderbdlebnd.dcx"))
            targets[Hash(p)] = p;
    }
    Console.Error.WriteLine($"[shaders] {targets.Count} candidate shader paths");
}
else
{
    var logicalPath = args[1];
    outRoot = args.Length > 2 ? args[2] : null;
    targets[Hash(logicalPath)] = logicalPath;
    Console.Error.WriteLine($"[target] {logicalPath} -> 0x{Hash(logicalPath):X}");
}

// ArchiveKeys is internal in Andre.Formats; reach GetKey(string, Game) via reflection.
var getKey = typeof(BinderArchive).Assembly.GetType("Andre.Formats.Util.ArchiveKeys")!
    .GetMethod("GetKey", BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static)!;
var fhType = typeof(BHD5).GetNestedType("FileHeader")!;
var hashProp = fhType.GetProperty("FileNameHash")!;
var readFile = fhType.GetMethod("ReadFile", new[] { typeof(FileStream) })!;

string[] archives = { "Data0", "Data1", "Data2", "Data3", "DLC" };
int found = 0;
foreach (var arc in archives)
{
    var bhdPath = Path.Combine(gameFolder, arc + ".bhd");
    var bdtPath = Path.Combine(gameFolder, arc + ".bdt");
    if (!File.Exists(bhdPath) || !File.Exists(bdtPath)) { Console.Error.WriteLine($"[skip] {arc}"); continue; }

    var key = (string)getKey.Invoke(null, new object[] { bhdPath, Game.ER })!;
    using var decStream = CryptographyUtility.DecryptRsa(bhdPath, key);
    var bhd = BHD5.Read(decStream.ToArray(), BHD5.Game.EldenRing);
    var buckets = (System.Collections.IEnumerable)typeof(BHD5).GetProperty("Buckets")!.GetValue(bhd)!;

    FileStream bdt = null;
    foreach (System.Collections.IEnumerable bucket in buckets)
        foreach (var h in bucket)
        {
            var fnh = Convert.ToUInt64(hashProp.GetValue(h));
            if (!targets.TryGetValue(fnh, out var path)) continue;
            found++;
            bdt ??= File.OpenRead(bdtPath);
            var data = (byte[])readFile.Invoke(h, new object[] { bdt })!;
            Report(arc, path, data, outRoot, verbose: !shadersMode);
        }
    bdt?.Dispose();
}
Console.Error.WriteLine($"[done] matched {found}/{targets.Count} target path(s) in archives");
return found > 0 ? 0 : 1;

static void Report(string arc, string logicalPath, byte[] raw, string outRoot, bool verbose = false)
{
    byte[] inner = raw;
    string innerMagic = Ascii(raw, 0, 4);
    if (raw.Length >= 3 && raw[0] == 'D' && raw[1] == 'C' && raw[2] == 'X')
    {
        inner = DCX.Decompress(raw).ToArray();
        innerMagic = Ascii(inner, 0, 4);
    }
    int memberCount = -1;
    if (BND4.Is(inner)) memberCount = BND4.Read(inner).Files.Count;
    Console.WriteLine($"{arc,-5} stored={raw.Length,-9} inner={inner.Length,-9} innerMagic={innerMagic,-6} members={memberCount,-5} {logicalPath}");

    if (verbose && memberCount >= 0)
    {
        Console.WriteLine($"{"idx",-4} {"size",-9} {"verdict",-10} {"chunks",-40} {"name"}");
        int i = 0;
        foreach (var f in BND4.Read(inner).Files)
        {
            var b = f.Bytes.ToArray();
            var (verdict, chunks) = ClassifyDx(b);
            Console.WriteLine($"{i,-4} {b.Length,-9} {verdict,-10} {chunks,-40} {f.Name}");
            i++;
        }
    }

    if (outRoot == null) return;
    var baseDir = Path.Combine(outRoot, logicalPath.TrimStart('/').Replace('/', '_'));
    Directory.CreateDirectory(baseDir);
    File.WriteAllBytes(Path.Combine(baseDir, "_container.bin"), inner);
    if (memberCount >= 0)
        foreach (var f in BND4.Read(inner).Files)
        {
            var rel = (f.Name ?? "unnamed").Replace('\\', '/').TrimStart('/');
            var dest = Path.Combine(baseDir, rel);
            Directory.CreateDirectory(Path.GetDirectoryName(dest)!);
            File.WriteAllBytes(dest, f.Bytes.ToArray());
        }
}

// Parse a D3D "DXBC" container (used for BOTH SM4/5 DXBC and SM6 DXIL) and
// classify by inner chunk FourCCs. DXIL/ILDB => SM6 (LLVM/DXIL); SHEX/SHDR => SM4/5 DXBC.
static (string verdict, string chunks) ClassifyDx(byte[] b)
{
    if (b == null || b.Length < 32 || b[0] != 'D' || b[1] != 'X' || b[2] != 'B' || b[3] != 'C')
        return ("non-DXBC", Ascii(b, 0, 4));
    uint count = BitConverter.ToUInt32(b, 28);
    var fourccs = new List<string>();
    bool dxil = false, sm5 = false;
    for (uint k = 0; k < count && 32 + k * 4 + 4 <= b.Length; k++)
    {
        int off = (int)BitConverter.ToUInt32(b, (int)(32 + k * 4));
        if (off < 0 || off + 8 > b.Length) continue;
        string cc = Ascii(b, off, 4);
        fourccs.Add(cc);
        if (cc is "DXIL" or "ILDB") dxil = true;
        if (cc is "SHEX" or "SHDR") sm5 = true;
    }
    string verdict = dxil ? "DXIL/SM6" : sm5 ? "DXBC/SM5" : "DXBC?";
    return (verdict, string.Join(",", fourccs));
}

static string Hex(byte[] b, int len)
{
    if (b == null) return "?";
    var s = new System.Text.StringBuilder();
    for (int i = 0; i < len && i < b.Length; i++) s.Append(b[i].ToString("x2"));
    return s.ToString();
}

static string Ascii(byte[] b, int off, int len)
{
    if (b == null || b.Length < off + len) return "?";
    var s = new System.Text.StringBuilder();
    for (int i = off; i < off + len; i++) s.Append(b[i] >= 0x20 && b[i] < 0x7f ? (char)b[i] : '.');
    return s.ToString();
}
