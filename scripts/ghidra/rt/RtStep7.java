import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

// Crack InGameStep step 7 (WorldResWait). Find the step handler table via xrefs to
// STEP_MoveMap_Init (dump 0x140aec210), read the table entries (step handlers), then
// decompile step 7 + the MoveMapStep/streaming path.
public class RtStep7 extends GhidraScript {
    DecompInterface di;
    void dec(long va, String tag) {
        Address a = toAddr(va);
        Function f = getFunctionContaining(a);
        println("################ " + tag + " " + Long.toHexString(va) + " -> " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "NO_FUNC") + " ################");
        if (f == null) return;
        DecompileResults r = di.decompileFunction(f, 200, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("(decompile failed: " + (r != null ? r.getErrorMessage() : "null") + ")");
    }
    void refsTo(long va, String tag) {
        println("=== refs TO " + tag + " " + Long.toHexString(va) + " ===");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(va));
        int n = 0;
        while (it.hasNext() && n < 60) {
            Reference r = it.next();
            Function f = getFunctionContaining(r.getFromAddress());
            MemoryBlock b = getMemoryBlock(r.getFromAddress());
            println("  from " + r.getFromAddress() + " [" + (b!=null?b.getName():"?") + "] " + r.getReferenceType() + "  " + (f != null ? f.getName()+" @ "+f.getEntryPoint() : "(data/no-func)"));
            n++;
        }
    }
    // Dump N pointers starting at a data address (the step-handler table).
    void ptrTable(long base, int count, String tag) {
        println("=== ptr table " + tag + " @ " + Long.toHexString(base) + " ===");
        for (int i = 0; i < count; i++) {
            try {
                long p = getLong(toAddr(base + (long)i*8));
                Function f = getFunctionContaining(toAddr(p));
                Symbol s = getSymbolAt(toAddr(p));
                println("  [" + i + "] = 0x" + Long.toHexString(p) + "  " + (f!=null?f.getName()+" @ "+f.getEntryPoint():(s!=null?s.getName():"?")));
            } catch (Exception e) { println("  [" + i + "] read err " + e); }
        }
    }
    @Override public void run() throws Exception {
        di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        String[] a = getScriptArgs();
        if (a.length == 0) {
            // default: locate the step table via MoveMap_Init refs
            refsTo(0x140aec210L, "STEP_MoveMap_Init");
            dec(0x140aec210L, "STEP_MoveMap_Init");
            dec(0x140aff730L, "MoveMapStep->orchestrator step");
            dec(0x140afb970L, "save orchestrator");
            dec(0x14066e2e4L, "STREAMING_ENABLE");
            return;
        }
        // args: op then addr[,count]
        String op = a[0];
        long addr = Long.decode(a[1]);
        if (op.equals("dec")) dec(addr, "arg");
        else if (op.equals("refs")) refsTo(addr, "arg");
        else if (op.equals("table")) ptrTable(addr, a.length>2?Integer.decode(a[2]):16, "arg");
    }
}
