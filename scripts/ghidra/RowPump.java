// Decompile the chain entry FUN_1409a7940 (-> FUN_1409a80d0 -> builder) and FUN_1409ac770
// (open_menu's direct sibling of the row leaf). Get xrefs to FUN_1409a7940. Decompile the
// job-post path FUN_1409aa580 / FUN_14078e1d0 / FUN_1407928c0 to see WHERE the chain installs
// (which object/offset = the pump target). Decompile FUN_1409a6dc0 (the first chained job).
// Usage: ghidra-query.sh scripts/ghidra/RowPump.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RowPump extends GhidraScript {
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
    void xrefs(long v, String label) {
        Address a = toAddr(v);
        Function f0 = getFunctionContaining(a);
        long entry = f0==null? v : f0.getEntryPoint().getOffset();
        println("---- XREFS TO " + label + " (entry 0x"+Long.toHexString(entry)+") ----");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(entry));
        boolean any=false;
        while (it.hasNext()) { any=true; Reference rf = it.next();
            Function ff = getFunctionContaining(rf.getFromAddress());
            println("  from 0x" + rf.getFromAddress() + " (" + (ff==null?"?":ff.getName()+" @0x"+ff.getEntryPoint()) + ") " + rf.getReferenceType());
        }
        if (!any) println("  (no xrefs)");
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x1409a7940L, "CHAIN ENTRY FUN_1409a7940");
        xrefs(0x1409a7940L, "CHAIN ENTRY FUN_1409a7940");
        dec(0x1409ac770L, "OPEN_MENU SIBLING FUN_1409ac770");
        dec(0x1409aa580L, "JOB POST FUN_1409aa580");
        dec(0x1409a6dc0L, "CHAINED JOB FUN_1409a6dc0");
    }
}
