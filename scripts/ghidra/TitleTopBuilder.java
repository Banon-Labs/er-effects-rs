import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class TitleTopBuilder extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label);
        println("VA(dump)=0x" + Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 90, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("  FAIL " + (r==null?"null":r.getErrorMessage()));
    }
    @Override public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        // FUN_14081fae0 (dump) = title TOP menu builder called by STEP_BeginTitle
        dec(0x14081fae0L, "TitleTopMenuBuilder(FUN_14081fae0)");
    }
}
