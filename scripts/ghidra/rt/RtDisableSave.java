import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.app.decompiler.*;
import java.util.*;

public class RtDisableSave extends GhidraScript {
    @Override public void run() throws Exception {
        int off = -1;
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        Iterator<Structure> it = dtm.getAllStructures();
        while (it.hasNext()) {
            Structure s = it.next();
            if (!s.getName().contains("CSMenuMan")) continue;
            for (DataTypeComponent c : s.getDefinedComponents()) {
                String fn = c.getFieldName();
                if (fn == null) continue;
                if (fn.equalsIgnoreCase("disableSaveMenu")) {
                    off = c.getOffset();
                    println("FIELD " + s.getName() + ".disableSaveMenu @ +0x" + Integer.toHexString(off) + " (" + c.getDataType().getName() + ")");
                } else if (fn.toLowerCase().contains("save") || fn.toLowerCase().contains("disable")) {
                    println("  (nearby) " + s.getName() + " +0x" + Integer.toHexString(c.getOffset()) + " " + fn);
                }
            }
        }
        Address glob = null;
        for (Symbol s : currentProgram.getSymbolTable().getSymbols("GLOBAL_CSMenuMan")) { println("GLOBAL_CSMenuMan @ " + s.getAddress()); glob = s.getAddress(); }
        if (off < 0 || glob == null) { println("(offset or global not found)"); return; }
        String needle = "+ 0x" + Integer.toHexString(off) + "],";
        LinkedHashSet<Function> funcs = new LinkedHashSet<>();
        ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(glob);
        while (ri.hasNext()) { Function f = getFunctionContaining(ri.next().getFromAddress()); if (f != null) funcs.add(f); }
        println("=== WRITES to [reg+0x" + Integer.toHexString(off) + "] (disableSaveMenu) in " + funcs.size() + " GLOBAL_CSMenuMan fns ===");
        DecompInterface di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        LinkedHashSet<Function> writers = new LinkedHashSet<>();
        for (Function f : funcs) {
            for (Instruction insn : currentProgram.getListing().getInstructions(f.getBody(), true)) {
                String t = insn.toString();
                if (t.contains(needle)) { println("  " + insn.getAddress() + " [" + f.getName() + "] " + t); writers.add(f); }
            }
        }
        // decompile up to 4 writer functions to see what condition sets it
        int shown = 0;
        for (Function f : writers) {
            if (shown++ >= 5) break;
            println("################ WRITER " + f.getName() + " @ " + f.getEntryPoint() + " ################");
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("(decompile failed)");
        }
    }
}
