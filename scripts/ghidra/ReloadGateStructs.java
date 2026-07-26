// ReloadGateStructs.java
// Dump struct layouts + resolve globals for the reload-gate chain.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.Address;
import java.util.*;

public class ReloadGateStructs extends GhidraScript {
    void dumpStruct(String type) {
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        Iterator<DataType> it = dtm.getAllDataTypes();
        int shown = 0;
        while (it.hasNext()) {
            DataType dt = it.next();
            if (!(dt instanceof Structure)) continue;
            if (!dt.getName().equalsIgnoreCase(type)) continue;
            Structure s = (Structure) dt;
            println("=== " + s.getName() + " size=0x" + Long.toHexString(s.getLength()) + " ===");
            for (DataTypeComponent c : s.getDefinedComponents()) {
                String nm = c.getFieldName();
                println("  +0x" + Long.toHexString(c.getOffset()) + "  " + (nm==null?"(unnamed)":nm)
                        + " : " + c.getDataType().getName());
            }
            if (++shown > 2) break;
        }
        if (shown == 0) println("no struct EXACT-matching " + type);
    }

    void resolveGlobal(String namesub) {
        SymbolTable st = currentProgram.getSymbolTable();
        SymbolIterator si = st.getSymbolIterator();
        int n = 0;
        while (si.hasNext() && n < 12) {
            Symbol s = si.next();
            if (s.getName().toLowerCase().contains(namesub.toLowerCase())) {
                println("  SYM " + s.getName() + " @ " + s.getAddress() + " (" + s.getSymbolType() + ")");
                n++;
            }
        }
        if (n == 0) println("  no symbol containing " + namesub);
    }

    public void run() throws Exception {
        println("###### STRUCTS ######");
        for (String t : new String[]{"MoveMapStep", "EzChildStepBase", "FD4StepTemplateBase", "InGameStep"}) {
            dumpStruct(t);
            println("");
        }
        println("###### GLOBALS ######");
        println("-- GLOBAL_CSRemo --"); resolveGlobal("GLOBAL_CSRemo");
        println("-- GLOBAL_CSSessionManager --"); resolveGlobal("GLOBAL_CSSessionManager");
        println("### DONE");
    }
}
