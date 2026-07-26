import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.app.decompiler.*;
import java.util.*;

// Find writers of CSMenuMan->disableSaveMenu (byte at +0x13c). Scans all instructions for
// byte-ptr immediate stores to [reg+0x13c], then decompiles the containing functions whose
// decompiled C actually references "disableSaveMenu" (so it's really the CSMenuMan field, not
// some other struct's +0x13c). Also name-searches for obvious setters.
public class RtSetter extends GhidraScript {
    @Override public void run() throws Exception {
        println("=== function names matching save/disable/menu-enter ===");
        SymbolIterator si = currentProgram.getSymbolTable().getAllSymbols(true);
        int nm = 0;
        while (si.hasNext() && nm < 60) {
            Symbol s = si.next(); String n = s.getName().toLowerCase();
            if (s.getSymbolType() == SymbolType.FUNCTION && (n.contains("disablesave") || n.contains("setdisable") || n.contains("savemenu") || n.contains("prohibitsave") || n.contains("enablesave"))) {
                println("  " + s.getAddress() + "  " + s.getName()); nm++;
            }
        }
        println("=== byte-ptr immediate stores to [reg+0x13c] ===");
        Listing lst = currentProgram.getListing();
        InstructionIterator ii = lst.getInstructions(true);
        LinkedHashSet<Function> cands = new LinkedHashSet<>();
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            String t = insn.toString();
            if (t.contains("+ 0x13c],") && t.toLowerCase().startsWith("mov byte")) {
                Function f = getFunctionContaining(insn.getAddress());
                println("  " + insn.getAddress() + " [" + (f != null ? f.getName() : "?") + "] " + t);
                if (f != null) cands.add(f);
            }
        }
        // decompile candidates that actually reference disableSaveMenu
        DecompInterface di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        int shown = 0;
        for (Function f : cands) {
            if (shown >= 8) break;
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r == null || !r.decompileCompleted()) continue;
            String c = r.getDecompiledFunction().getC();
            if (c.contains("disableSaveMenu")) {
                println("################ SETTER " + f.getName() + " @ " + f.getEntryPoint() + " ################");
                println(c);
                shown++;
            }
        }
        println("(scan done, " + cands.size() + " candidate fns)");
    }
}
