// Confirm FUN_140e85f60 (the pump's "open menu" trigger getter) and decompile it; check the
// tfc+0x14c gate semantics in the row-build mid FUN_1409a92a0 path.
// Usage: ghidra-query.sh scripts/ghidra/AcceptGate.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class AcceptGate extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 160, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x140e85f60L, "PUMP TRIGGER GETTER FUN_140e85f60");
        // FUN_1407a72f0 / FUN_1407a7340 = the Continue-vs-noop branch result builders in the mid:
        dec(0x1407a72f0L, "MID BRANCH A FUN_1407a72f0");
        dec(0x1407a7340L, "MID BRANCH B FUN_1407a7340 (noop success)");
    }
}
