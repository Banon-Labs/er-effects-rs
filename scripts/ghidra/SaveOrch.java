// Decompile orchestrator + the title-flow step builders. Larger timeout, defensive print.
// Usage: ghidra-query.sh scripts/ghidra/SaveOrch.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class SaveOrch extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 240, monitor);
        if (r == null) { println("  null results"); return; }
        if (!r.decompileCompleted()) { println("  FAILED: " + r.getErrorMessage()); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df == null) { println("  null decompiled fn"); return; }
        String c = df.getC();
        println("  len="+c.length());
        println(c);
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        DecompileOptions opts = new DecompileOptions();
        di.setOptions(opts);
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x14082f850L, "ORCHESTRATOR");
    }
}
