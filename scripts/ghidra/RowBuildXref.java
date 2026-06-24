// Xref-trace upward from the row-build leaf to find what triggers it, and check
// whether open_menu (0x1409b2630 dump) reaches it. Also decompile FUN_1409a92a0 (the
// real mid called by the builder entry) and get xrefs to builder entry 0x1409a71a0,
// FUN_1409a92a0, and the leaf 0x1409ac8b0.
// Usage: ghidra-query.sh scripts/ghidra/RowBuildXref.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RowBuildXref extends GhidraScript {
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
        while (it.hasNext()) {
            any=true;
            Reference rf = it.next();
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
        xrefs(0x1409a71a0L, "BUILDER ENTRY FUN_1409a71a0");
        dec(0x1409a92a0L, "REAL MID FUN_1409a92a0");
        xrefs(0x1409a92a0L, "REAL MID FUN_1409a92a0");
        xrefs(0x1409ac8b0L, "LEAF FUN_1409ac8b0");
    }
}
