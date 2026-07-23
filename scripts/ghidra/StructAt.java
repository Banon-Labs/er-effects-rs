// StructAt.java <StructName> <0xOFFSET>
// Resolve a struct by name (search symbol/datatype by exact name via DataTypeManager.getDataType paths
// using a guarded iteration that skips BadDataType), then print the component at/containing OFFSET,
// plus a window of components around it.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;
import java.util.*;

public class StructAt extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        String name = a[0];
        long off = Long.decode(a[1]);
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        ArrayList<DataType> all = new ArrayList<>();
        dtm.getAllStructures().forEachRemaining(s -> all.add((DataType)s));
        Structure target = null;
        for (DataType dt : all) {
            if (dt.getName().equals(name)) { target = (Structure)dt; break; }
        }
        if (target == null) {
            for (DataType dt : all) if (dt.getName().contains(name)) { target = (Structure)dt; println("(substr match: "+dt.getName()+")"); break; }
        }
        if (target == null) { println("no struct " + name); return; }
        println("STRUCT " + target.getName() + " size=0x"+Integer.toHexString(target.getLength()));
        DataTypeComponent c = target.getComponentContaining((int)off);
        println("Component containing +0x"+Long.toHexString(off)+": " +
            (c==null?"(none/undefined)":("+0x"+Integer.toHexString(c.getOffset())+" "+c.getDataType().getName()+" "+c.getFieldName())));
        // window
        for (DataTypeComponent comp : target.getDefinedComponents()) {
            int o = comp.getOffset();
            if (o >= off-0x40 && o <= off+0x40)
                println(String.format("  +0x%x %-40s %s", o, comp.getDataType().getName(), comp.getFieldName()==null?"":comp.getFieldName()));
        }
    }
}
