// Second pass: SetState dispatcher, store helper, and where the real menu list is built.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class TitleMenuDecomp2 extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label);
        println("VA(dump)=0x" + Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 60, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("  FAIL " + (r==null?"null":r.getErrorMessage()));
    }
    @Override public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        // FUN_140b0da50 = the SetState used by STEP (dump VA, from STEP decomp)
        dec(0x140b0da50L, "STEP_SetState(FUN_140b0da50)");
        // FUN_140b0e620 = store-built-job helper
        dec(0x140b0e620L, "store_job(FUN_140b0e620)");
        // Now: find callers/xrefs of STEP_BeginLogo's owner to find the OTHER STEP funcs.
        // List all functions named STEP_* to locate the menu builder step.
        FunctionManager fm = currentProgram.getFunctionManager();
        println("==================================================== STEP_* functions");
        for (Function f : fm.getFunctions(true)) {
            String n = f.getName();
            if (n.startsWith("STEP_") || n.contains("TitleStep") || n.contains("TitleMenu")
                || n.contains("TitleTop") || n.contains("MenuTitle")) {
                println("  " + n + " @ 0x" + f.getEntryPoint());
            }
        }
    }
}
