// Trace what triggers the row-build job chain: decompile FUN_1409a80d0 (calls builder
// entry FUN_1409a71a0) and its xrefs; identify the vtable that holds FUN_1409a71a0 (slot
// at 0x14490236c) -> which MenuJob class + which vtable index; check open_menu (0x1409b2630)
// body for what job it queues.
// Usage: ghidra-query.sh scripts/ghidra/RowTrigger.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class RowTrigger extends GhidraScript {
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
    // Identify the symbol/namespace of the object whose vtable contains the data-ref address.
    void vtblOwner(long dataRef, String label) {
        println("---- VTABLE-SLOT OWNER for " + label + " @0x"+Long.toHexString(dataRef)+" ----");
        // Walk backwards to find the vtable head (a symbol). Print nearby symbols.
        Address a = toAddr(dataRef);
        Symbol s = getSymbolAt(a);
        println("  symbolAt=" + (s==null?"<none>":s.getName()));
        // print the primary symbol within 0x200 before
        for (long off=0; off<0x400; off+=8) {
            Symbol ps = getSymbolAt(toAddr(dataRef-off));
            if (ps != null && !ps.getName().startsWith("DAT_") && !ps.getName().startsWith("PTR_")) {
                println("  head -0x"+Long.toHexString(off)+" = " + ps.getName());
                break;
            }
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x1409a80d0L, "CALLER FUN_1409a80d0 (calls builder)");
        xrefs(0x1409a80d0L, "CALLER FUN_1409a80d0");
        vtblOwner(0x14490236cL, "builder-entry slot");
        dec(0x1409b2630L, "OPEN_MENU registrar 0x1409b24e0");
    }
}
