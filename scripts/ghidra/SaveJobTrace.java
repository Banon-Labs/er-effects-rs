// Trace the save-update MenuJob: factory FUN_140826c30 callers, the job ctor
// FUN_1408278c0, the lambda vftable, and decompile them. Find the Run() and outcome branch.
// Usage: ghidra-query.sh scripts/ghidra/SaveJobTrace.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class SaveJobTrace extends GhidraScript {
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
        else println("  DECOMPILE FAILED");
    }
    void xrefs(long v, String label) {
        Address a = toAddr(v);
        println("---- XREFS TO " + label + " 0x" + Long.toHexString(v) + " ----");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(a);
        while (it.hasNext()) {
            Reference rf = it.next();
            Address from = rf.getFromAddress();
            Function ff = getFunctionContaining(from);
            println("  from 0x" + from + " (" + (ff==null?"?":ff.getName()) + ") " + rf.getReferenceType());
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        xrefs(0x140826c30L, "factory FUN_140826c30");
        dec(0x1408278c0L, "job-ctor FUN_1408278c0");
        // the lambda vftable address holds the Func_impl invoke ptr; find xrefs to ctor too
        xrefs(0x1408278c0L, "job-ctor");
    }
}
