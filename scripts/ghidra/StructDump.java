// StructDump.java <StructName> [<StructName>...]
// Print the field layout of named structures (data types) from the program's type manager.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;
import java.util.*;

public class StructDump extends GhidraScript {
    @Override
    public void run() throws Exception {
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        for (String name : getScriptArgs()) {
            ArrayList<DataType> hits = new ArrayList<>();
            Iterator<DataType> it = dtm.getAllDataTypes();
            while (it.hasNext()) {
                DataType dt = it.next();
                if (dt.getName().equals(name) || dt.getName().contains(name)) hits.add(dt);
            }
            for (DataType dt : hits) {
                if (!(dt instanceof Structure)) { println("== " + dt.getName() + " (not struct: " + dt.getClass().getSimpleName() + ")"); continue; }
                Structure s = (Structure) dt;
                println("== STRUCT " + s.getName() + " size=0x" + Integer.toHexString(s.getLength()));
                for (DataTypeComponent c : s.getDefinedComponents()) {
                    println(String.format("  +0x%x  %-30s %s", c.getOffset(),
                        c.getDataType().getName(), c.getFieldName()==null?"":c.getFieldName()));
                }
            }
            if (hits.isEmpty()) println("(no datatype matching " + name + ")");
        }
    }
}
