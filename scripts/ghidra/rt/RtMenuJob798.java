import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.*;

// Understand the post-load bounce: STEP_RequestWait ends the in-game session when CSMenuMan+0x798
// (in-game menu job) reads 0. Decompile the step fns + find what WRITES CSMenuMan+0x798 (populates the job).
public class RtMenuJob798 extends GhidraScript {
    DecompInterface di;
    void dec(long va, String tag) {
        Function f = getFunctionContaining(toAddr(va));
        println("################ " + tag + " " + Long.toHexString(va) + " -> " + (f!=null?f.getName():"?") + " ################");
        if (f==null) return;
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        if (r!=null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("(decompile failed)");
    }
    @Override public void run() throws Exception {
        di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        // 1) the two step fns from return_title.rs notes
        dec(0x140aecd00L, "STEP_RequestWait");
        dec(0x140b0ced0L, "STEP_GameStepWait");
        // 2) writers of CSMenuMan+0x798 : scan fns that reference GLOBAL_CSMenuMan (0x143d6b7b0) for a store to [reg+0x798]
        Address glob = toAddr(0x143d6b7b0L);
        LinkedHashSet<Function> funcs = new LinkedHashSet<>();
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(glob);
        while (it.hasNext()) { Function f = getFunctionContaining(it.next().getFromAddress()); if (f!=null) funcs.add(f); }
        println("################ WRITES to [reg+0x798] in " + funcs.size() + " GLOBAL_CSMenuMan fns ################");
        LinkedHashSet<Function> writers = new LinkedHashSet<>();
        for (Function f : funcs) {
            for (Instruction insn : currentProgram.getListing().getInstructions(f.getBody(), true)) {
                String t = insn.toString();
                if (t.contains("+ 0x798],") && insn.getMnemonicString().toLowerCase().startsWith("mov")) {
                    println("  " + insn.getAddress() + " [" + f.getName() + "] " + t); writers.add(f);
                }
            }
        }
        int shown=0;
        for (Function f : writers) { if (shown++>=3) break; dec(f.getEntryPoint().getOffset(), "WRITER"); }
    }
}
