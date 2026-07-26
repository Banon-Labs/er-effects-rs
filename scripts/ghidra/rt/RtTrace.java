import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.List;

public class RtTrace extends GhidraScript {
    DecompInterface di;
    void dec(Function f) {
        if (f == null) { println("(no func)"); return; }
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        if (r == null || !r.decompileCompleted()) { println("(decompile failed: " + (r != null ? r.getErrorMessage() : "null") + ")"); return; }
        println(r.getDecompiledFunction().getC());
    }
    void callers(long entry) {
        Address a = toAddr(entry);
        println("--- callers of " + a + " ---");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(a);
        int n = 0;
        while (it.hasNext()) {
            Reference r = it.next();
            if (r.getReferenceType().isCall() || r.getReferenceType().isJump()) {
                Function f = getFunctionContaining(r.getFromAddress());
                println("  " + r.getFromAddress() + "  " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "?") + "  " + r.getReferenceType());
                n++;
            }
        }
        if (n == 0) println("  (no call refs)");
    }
    @Override public void run() throws Exception {
        di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        for (String nm : new String[]{"CanShowSaveMenu", "STEP_ReturnTitle", "ShouldSave"}) {
            List<Function> fs = getGlobalFunctions(nm);
            println("################ " + nm + " -> " + (fs.isEmpty() ? "NOT FOUND" : fs.get(0).getEntryPoint()) + " ################");
            if (!fs.isEmpty()) dec(fs.get(0));
        }
        callers(0x14067b840L);
        callers(0x14067ba30L);
        callers(0x14067aa70L);
    }
}
