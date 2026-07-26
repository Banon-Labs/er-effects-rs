import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.Function;

public class RtGates extends GhidraScript {
    DecompInterface di;
    void dec(long va) {
        Function f = getFunctionContaining(toAddr(va));
        println("################ " + Long.toHexString(va) + " -> " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "NO_FUNC") + " ################");
        if (f == null) return;
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("(decompile failed)");
    }
    @Override public void run() throws Exception {
        di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        dec(0x14067a170L); // orchestrator pre-save gate: if(!this) return
        dec(0x14080d660L); // ShouldSave secondary gate
        dec(0x140aff730L); // MoveMapStep step that calls the orchestrator (does it gate the call?)
        dec(0x140679460L); // bVar2 = FUN_140679460 (the co-op/other save condition, reads bc4)
    }
}
