using System.Reflection;
using System.Runtime.Loader;

var sb = Environment.GetEnvironmentVariable("SMITHBOX_BINARY_DIR")!;
AssemblyLoadContext.Default.Resolving += (ctx, name) =>
{
    if (name.Name is null) return null;
    var cand = Path.Combine(sb, name.Name + ".dll");
    return File.Exists(cand) ? ctx.LoadFromAssemblyPath(cand) : null;
};
string Pretty(Type t)
{
    if (t.IsByRef) return Pretty(t.GetElementType()) + "&";
    if (t.IsPointer) return Pretty(t.GetElementType()) + "*";
    if (t.IsArray) return Pretty(t.GetElementType()) + "[]";
    if (t.IsGenericType) return $"{t.Name.Split('`')[0]}<{string.Join(",", t.GetGenericArguments().Select(Pretty))}>";
    return t.Name;
}
string Sig(MethodBase m)
{
    var ps = string.Join(", ", m.GetParameters().Select(p => $"{Pretty(p.ParameterType)} {p.Name}"));
    var ret = m is MethodInfo mi ? Pretty(mi.ReturnType) + " " : "";
    var imp = (m.Attributes & MethodAttributes.PinvokeImpl) != 0 ? "[PInvoke] " : "";
    return $"{imp}{(m.IsStatic ? "static " : "")}{ret}{m.Name}({ps})";
}
var flags = BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.Static | BindingFlags.DeclaredOnly;
var asm = AssemblyLoadContext.Default.LoadFromAssemblyPath(Path.Combine(sb, "Andre.SoulsFormats.dll"));
foreach (var tn in new[] { "SoulsFormats.Oodle", "SoulsFormats.Oodle26", "SoulsFormats.Oodle28", "SoulsFormats.Oodle29" })
{
    var t = asm.GetType(tn);
    if (t == null) continue;
    Console.WriteLine($"\n##### {t.FullName} #####");
    foreach (var m in t.GetMethods(flags).Where(m => !m.IsSpecialName))
    {
        Console.WriteLine($"   {Sig(m)}");
        if ((m.Attributes & MethodAttributes.PinvokeImpl) != 0)
        {
            var imp = m.GetCustomAttributesData().FirstOrDefault(a => a.AttributeType.Name == "DllImportAttribute");
            if (imp != null) Console.WriteLine($"        DllImport={imp.ConstructorArguments[0].Value} EntryPoint={(m.GetCustomAttributesData().SelectMany(a=>a.NamedArguments).FirstOrDefault(n=>n.MemberName=="EntryPoint").TypedValue.Value) ?? m.Name}");
        }
    }
    foreach (var nt in t.GetNestedTypes(flags)) Console.WriteLine($"   NESTED {nt.Name}");
}
