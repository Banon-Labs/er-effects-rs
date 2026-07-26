// EnumFieldDump.java <StructNameSubstr> <fieldNameSubstr> [<EnumNameSubstr>]
// 1) Print matching struct fields (offset/name/type) filtered by fieldNameSubstr.
// 2) Print every enum whose name contains EnumNameSubstr (or, if omitted, whose name
//    contains the fieldNameSubstr), listing name=value pairs.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;
import java.util.*;

public class EnumFieldDump extends GhidraScript {
    public void run() throws Exception {
        String[] a = getScriptArgs();
        String structNeedle = a.length > 0 ? a[0].toLowerCase() : null;
        String fieldNeedle  = a.length > 1 ? a[1].toLowerCase() : null;
        String enumNeedle   = a.length > 2 ? a[2].toLowerCase() : (fieldNeedle);
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        java.util.Iterator<DataType> it = dtm.getAllDataTypes();
        int structsShown = 0, enumsShown = 0;
        while (it.hasNext()) {
            DataType dt = it.next();
            String dn = dt.getName().toLowerCase();
            if (dt instanceof Structure && structNeedle != null && dn.contains(structNeedle)) {
                Structure s = (Structure) dt;
                boolean any = false;
                for (DataTypeComponent c : s.getDefinedComponents()) {
                    String nm = c.getFieldName();
                    if (fieldNeedle != null && (nm == null || !nm.toLowerCase().contains(fieldNeedle))) continue;
                    if (!any) { println("=== STRUCT " + s.getName() + " size=0x" + Long.toHexString(s.getLength()) + " ==="); any = true; }
                    DataType fdt = c.getDataType();
                    println("  +0x" + Long.toHexString(c.getOffset()) + "  " + nm + " : " + fdt.getName());
                    // If the field is (or wraps) an Enum, dump its members inline.
                    DataType base = fdt;
                    while (base instanceof TypeDef) base = ((TypeDef) base).getBaseDataType();
                    if (base instanceof ghidra.program.model.data.Enum) {
                        ghidra.program.model.data.Enum e = (ghidra.program.model.data.Enum) base;
                        println("     ENUM " + e.getName() + " size=" + e.getLength() + ":");
                        for (String en : e.getNames()) println("       " + en + " = " + e.getValue(en));
                    }
                }
                if (any) ++structsShown;
            }
            if (dt instanceof ghidra.program.model.data.Enum && enumNeedle != null && dn.contains(enumNeedle)) {
                ghidra.program.model.data.Enum e = (ghidra.program.model.data.Enum) dt;
                println("=== ENUM " + e.getName() + " size=" + e.getLength() + " ===");
                for (String nm : e.getNames()) {
                    println("  " + nm + " = " + e.getValue(nm));
                }
                if (++enumsShown > 8) break;
            }
        }
        if (structsShown == 0) println("(no struct field matches for " + structNeedle + "/" + fieldNeedle + ")");
        if (enumsShown == 0) println("(no enum matches for " + enumNeedle + ")");
    }
}
