// Decompile the save-update GATE FUN_14082d090, the alternate no-op job FUN_1407a7340,
// and the job ctor + lambda Run. Defensive multi-line print.
// Usage: ghidra-query.sh scripts/ghidra/SaveGate.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class SaveGate extends GhidraScript {
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
        if (df == null) { println("  null df"); return; }
        println(df.getC());
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x14082d090L, "SAVE-UPDATE GATE FUN_14082d090");
        dec(0x1407a7340L, "ALT NO-OP JOB FUN_1407a7340");
        dec(0x1408278c0L, "JOB CTOR FUN_1408278c0");
    }
}
