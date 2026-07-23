// Decompile the native Continue/Load/NewGame row-build chain + xrefs to the builder.
// dump VAs (deobf+0x150): builder 0x1409a71c0, mid 0x1409a9260, leaf 0x1409ac8b0,
// open_menu 0x1409b2630, press-accept 0x1409b13b0.
// Usage: ghidra-query.sh scripts/ghidra/RowBuild.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RowBuild extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 220, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    void xrefs(long v, String label) {
        println("---- XREFS TO " + label + " 0x" + Long.toHexString(v) + " ----");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(v));
        while (it.hasNext()) {
            Reference rf = it.next();
            Function ff = getFunctionContaining(rf.getFromAddress());
            println("  from 0x" + rf.getFromAddress() + " (" + (ff==null?"?":ff.getName()) + ") " + rf.getReferenceType());
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        xrefs(0x1409a71c0L, "BUILDER FUN_1409a7070");
        dec(0x1409a71c0L, "BUILDER FUN_1409a7070");
        dec(0x1409a9260L, "MID FUN_1409a9110");
        dec(0x1409ac8b0L, "LEAF 0x1409ac760");
    }
}
