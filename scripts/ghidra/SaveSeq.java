// Decompile the TitleFlow step dispatcher FUN_14082dc60 (sequences the steps incl save-update),
// find writes to TitleFlowContext+0x148 (save regulationVersion source), and resolve the
// CSRegulationManager singleton global address + the +0x44 read site.
// Usage: ghidra-query.sh scripts/ghidra/SaveSeq.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class SaveSeq extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 200, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    void sym(String name) {
        SymbolIterator it = currentProgram.getSymbolTable().getSymbols(name);
        while (it.hasNext()) {
            Symbol s = it.next();
            println("  SYMBOL " + name + " @ 0x" + s.getAddress());
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        sym("GLOBAL_CSRegulationManager");
        dec(0x14082dc60L, "STEP DISPATCHER FUN_14082dc60");
    }
}
