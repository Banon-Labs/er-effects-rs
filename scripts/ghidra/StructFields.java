// StructFields.java <TypeName> [<fieldNameSubstr>]
// Prints field offsets/names/types of a named structure; optional filter substring.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;

public class StructFields extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        String type = args[0];
        String filt = args.length > 1 ? args[1].toLowerCase() : null;
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        java.util.Iterator<DataType> it = dtm.getAllDataTypes();
        int shown = 0;
        while (it.hasNext()) {
            DataType dt = it.next();
            if (!(dt instanceof Structure)) continue;
            if (!dt.getName().equalsIgnoreCase(type) && !dt.getName().toLowerCase().contains(type.toLowerCase())) continue;
            Structure s = (Structure) dt;
            println("=== " + s.getName() + " size=0x" + Long.toHexString(s.getLength()) + " ===");
            for (DataTypeComponent c : s.getDefinedComponents()) {
                String nm = c.getFieldName();
                if (filt != null && (nm == null || !nm.toLowerCase().contains(filt))) continue;
                println("  +0x" + Long.toHexString(c.getOffset()) + "  " + (nm==null?"(unnamed)":nm)
                        + " : " + c.getDataType().getName());
            }
            if (++shown > 4) break;
        }
        if (shown == 0) println("no struct matching " + type);
    }
}
