// Decompile the save-update orchestrator FUN_14082f850 (caller of the factory),
// the job ctor FUN_1408278c0, and the vftable around 0x1448e8bc0 to ID the job class + Run.
// Usage: ghidra-query.sh scripts/ghidra/SaveJobTrace2.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class SaveJobTrace2 extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label);
        println("VA(dump)=0x" + Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("  DECOMPILE FAILED: " + (r==null?"null":r.getErrorMessage()));
    }
    void vtbl(long base, int n, String label) {
        println("---- VTABLE " + label + " @0x" + Long.toHexString(base) + " ----");
        Memory mem = currentProgram.getMemory();
        for (int i=0;i<n;i++) {
            try {
                long p = mem.getLong(toAddr(base + (long)i*8));
                Function ff = getFunctionAt(toAddr(p));
                if (ff==null) ff = getFunctionContaining(toAddr(p));
                println("  [+0x"+Integer.toHexString(i*8)+"] -> 0x"+Long.toHexString(p)+
                        (ff==null?"":(" "+ff.getName())));
            } catch (Exception e) { println("  [+0x"+Integer.toHexString(i*8)+"] <err>"); }
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        dec(0x14082f850L, "ORCHESTRATOR FUN_14082f850 (calls factory)");
        dec(0x1408278c0L, "JOB CTOR FUN_1408278c0");
        // vftable region the factory's DATA xref came from (0x1448e8bc0). Walk a window before/after.
        vtbl(0x1448e8b80L, 24, "near 0x1448e8bc0");
    }
}
