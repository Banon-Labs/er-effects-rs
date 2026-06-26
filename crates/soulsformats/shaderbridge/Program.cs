// er-shaderbridge: the win-x64/wine SoulsFormats worker for Elden Ring shaders.
//
// Why a separate bridge from crates/soulsformats/bridge (param-rows): shader
// containers are Oodle-Kraken (DCX-KRAK) compressed, and the only available Oodle
// decompressor is the game's Windows oo2core_*.dll. So this worker is published
// win-x64 and run under wine, where that DLL loads natively. The param-rows bridge
// stays host-native and untouched.
//
// Verbs (all emit one JSON line on stdout; diagnostics go to stderr):
//   survey  <gameDir>                         -> [{path,archive,storedBytes,innerBytes,innerMagic,members}]
//   extract <gameDir> <logicalPath> <outDir>  -> {path,archive,members:[{name,size}]}  (writes member files)

using System.Reflection;
using System.Runtime.Loader;
using System.Text.Json;
using Andre.Core;
using Andre.Formats;
using SoulsFormats;
using SoulsFormats.Util;

const int Ok = 0, Fail = 1, Usage = 2;

string smithboxDir = Environment.GetEnvironmentVariable("SMITHBOX_BINARY_DIR");
if (string.IsNullOrEmpty(smithboxDir))
{
    Console.Error.WriteLine("SMITHBOX_BINARY_DIR must point at the Smithbox install (Andre.*.dll, oo2core, Assets).");
    return Fail;
}
// Transitive Andre deps + the Oodle DLL + the UXM dictionary all resolve relative
// to the Smithbox install, so load missing (non-framework) assemblies from there
// and run with it as the working directory.
AssemblyLoadContext.Default.Resolving += (ctx, name) =>
{
    if (name.Name is null || name.Name.StartsWith("System.")) return null;
    var cand = Path.Combine(smithboxDir, name.Name + ".dll");
    return File.Exists(cand) ? ctx.LoadFromAssemblyPath(cand) : null;
};
Directory.SetCurrentDirectory(smithboxDir);

return Run(args);

static int Run(string[] args)
{
    if (args.Length < 2) { Usage(); return 2; }
    string verb = args[0], gameDir = args[1];
    return verb switch
    {
        "survey" => Survey(gameDir),
        "extract" when args.Length >= 4 => Extract(gameDir, args[2], args[3]),
        _ => Usage(),
    };

    static int Usage()
    {
        Console.Error.WriteLine("usage: er-shaderbridge survey <gameDir>");
        Console.Error.WriteLine("       er-shaderbridge extract <gameDir> <logicalPath> <outDir>");
        return 2;
    }
}

// --- shared archive plumbing -------------------------------------------------

// ER BHD5 path hash: 64-bit, hash = hash*0x85 + char over the normalized path
// (forward slashes, leading '/', lowercased). Matches Andre.Core BhdDictionary.
static ulong Hash(string logicalPath)
{
    string n = logicalPath.Trim().Replace('\\', '/').ToLowerInvariant();
    if (!n.StartsWith('/')) n = "/" + n;
    ulong h = 0;
    foreach (char c in n) h = h * 0x85ul + c;
    return h;
}

// Invoke each present (Data*/DLC) archive's BHD5 + BDT, calling `onFile` for any
// FileHeader whose hash is in `wanted` (hash -> logical path).
static void ForEachWantedFile(string gameDir, Dictionary<ulong, string> wanted,
    Action<string, string, byte[]> onFile)
{
    string[] archives = { "Data0", "Data1", "Data2", "Data3", "DLC" };
    var getKey = typeof(BinderArchive).Assembly.GetType("Andre.Formats.Util.ArchiveKeys")!
        .GetMethod("GetKey", BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Static)!;
    var fhType = typeof(BHD5).GetNestedType("FileHeader")!;
    var hashProp = fhType.GetProperty("FileNameHash")!;
    var readFile = fhType.GetMethod("ReadFile", new[] { typeof(FileStream) })!;
    var bucketsProp = typeof(BHD5).GetProperty("Buckets")!;

    foreach (var arc in archives)
    {
        string bhd = Path.Combine(gameDir, arc + ".bhd"), bdt = Path.Combine(gameDir, arc + ".bdt");
        if (!File.Exists(bhd) || !File.Exists(bdt)) continue;
        string key = (string)getKey.Invoke(null, new object[] { bhd, Game.ER })!;
        using var dec = CryptographyUtility.DecryptRsa(bhd, key);
        var bhd5 = BHD5.Read(dec.ToArray(), BHD5.Game.EldenRing);
        FileStream bdtStream = null;
        try
        {
            foreach (System.Collections.IEnumerable bucket in (System.Collections.IEnumerable)bucketsProp.GetValue(bhd5)!)
                foreach (var h in bucket)
                {
                    ulong fnh = Convert.ToUInt64(hashProp.GetValue(h));
                    if (!wanted.TryGetValue(fnh, out var path)) continue;
                    bdtStream ??= File.OpenRead(bdt);
                    onFile(arc, path, (byte[])readFile.Invoke(h, new object[] { bdtStream })!);
                }
        }
        finally { bdtStream?.Dispose(); }
    }
}

static byte[] Inner(byte[] raw)
    => raw.Length >= 3 && raw[0] == 'D' && raw[1] == 'C' && raw[2] == 'X' ? DCX.Decompress(raw).ToArray() : raw;

static string Ascii(byte[] b, int n)
{
    if (b == null || b.Length < n) return "";
    var s = new System.Text.StringBuilder();
    for (int i = 0; i < n; i++) s.Append(b[i] >= 0x20 && b[i] < 0x7f ? (char)b[i] : '.');
    return s.ToString();
}

// --- verbs -------------------------------------------------------------------

static int Survey(string gameDir)
{
    // Every dictionary path that looks like a shader container.
    var dict = Path.Combine(Directory.GetCurrentDirectory(), "Assets", "UXM Dictionaries", "EldenRingDictionary.txt");
    var wanted = new Dictionary<ulong, string>();
    foreach (var line in File.ReadLines(dict))
    {
        var p = line.Trim();
        if (p.Length == 0) continue;
        if (p.Contains("shader", StringComparison.OrdinalIgnoreCase)
            || p.EndsWith(".shaderbnd.dcx") || p.EndsWith(".shaderbdlebnd.dcx"))
            wanted[Hash(p)] = p;
    }
    var rows = new List<object>();
    ForEachWantedFile(gameDir, wanted, (arc, path, raw) =>
    {
        var inner = Inner(raw);
        int members = BND4.Is(inner) ? BND4.Read(inner).Files.Count : -1;
        rows.Add(new { path, archive = arc, storedBytes = raw.Length, innerBytes = inner.Length, innerMagic = Ascii(inner, 4), members });
    });
    Console.WriteLine(JsonSerializer.Serialize(rows));
    return rows.Count > 0 ? 0 : 1;
}

static int Extract(string gameDir, string logicalPath, string outDir)
{
    var wanted = new Dictionary<ulong, string> { [Hash(logicalPath)] = logicalPath };
    object result = null;
    ForEachWantedFile(gameDir, wanted, (arc, path, raw) =>
    {
        var inner = Inner(raw);
        if (!BND4.Is(inner)) throw new InvalidOperationException($"inner payload is not BND4 (magic={Ascii(inner, 4)})");
        Directory.CreateDirectory(outDir);
        var members = new List<object>();
        foreach (var f in BND4.Read(inner).Files)
        {
            var bytes = f.Bytes.ToArray();
            var name = (f.Name ?? "unnamed").Replace('\\', '/');
            // Member names can be full Windows roots (e.g. "N:\...\c4800_Body.matbin").
            // Flatten to a single filename: the '/' -> '_' join plus stripping the
            // drive ':' keeps Path.Combine from treating "N:..." as a rooted path
            // and silently discarding outDir.
            var safe = name.Replace('/', '_').Replace(':', '_');
            var dest = Path.Combine(outDir, safe);
            File.WriteAllBytes(dest, bytes);
            members.Add(new { name, size = bytes.Length, file = Path.GetFileName(dest) });
        }
        result = new { path, archive = arc, outDir, members };
    });
    if (result is null) { Console.Error.WriteLine($"not found in any archive: {logicalPath}"); return 1; }
    Console.WriteLine(JsonSerializer.Serialize(result));
    return 0;
}
