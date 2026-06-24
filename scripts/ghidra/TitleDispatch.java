import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class TitleDispatch extends GhidraScript {
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
        // Find xrefs to STEP_BeginLogo (0x140b0c390) to see the dispatch table / pump that calls it by state.
        Address logo = toAddr(0x140b0c390L);
        println("==== XREFS to STEP_BeginLogo 0x140b0c390 ====");
        for (Reference r : getReferencesTo(logo)) {
            println("  from 0x" + r.getFromAddress() + " type=" + r.getReferenceType());
        }
        // What references the singleton 0x144588e98? Identify its class.
        Address sing = toAddr(0x144588e98L);
        println("==== XREFS to singleton 0x144588e98 (first 12) ====");
        int n=0;
        for (Reference r : getReferencesTo(sing)) {
            println("  from 0x" + r.getFromAddress() + " type=" + r.getReferenceType());
            if(++n>=12) break;
        }
        // the TitleStep pump / update that dispatches by currentState
        dec(0x140b0d640L, "STEP_NextLap");
        dec(0x140b0ced0L, "STEP_GameStepWait");
    }
}
