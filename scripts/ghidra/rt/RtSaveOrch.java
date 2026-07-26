import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RtSaveOrch extends GhidraScript {
    DecompInterface di;
    void dec(long va) {
        Function f = getFunctionContaining(toAddr(va));
        println("################ " + Long.toHexString(va) + " -> " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "NO_FUNC") + " ################");
        if (f == null) return;
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("(decompile failed: " + (r != null ? r.getErrorMessage() : "null") + ")");
    }
    void callers(long entry) {
        println("--- callers of " + Long.toHexString(entry) + " ---");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(entry));
        int n = 0;
        while (it.hasNext()) {
            Reference r = it.next();
            if (r.getReferenceType().isCall() || r.getReferenceType().isJump()) {
                Function f = getFunctionContaining(r.getFromAddress());
                println("  " + r.getFromAddress() + "  " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "?"));
                n++;
            }
        }
        if (n == 0) println("  (none)");
    }
    @Override public void run() throws Exception {
        di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        dec(0x140afb970L); // save orchestrator (calls both 1->2 pumps)
        dec(0x140afbbc0L); // DoSaveStuff (calls 2->3 pump)
        callers(0x140afb970L);
        callers(0x140afbbc0L);
    }
}
