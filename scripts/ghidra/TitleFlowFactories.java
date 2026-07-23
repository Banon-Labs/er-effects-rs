// Decompile the TitleFlowContext job factories chained by FUN_140833970 (open_menu's flow graph)
// to determine whether any builds the Continue/Load/NewGame menu-item windows.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class TitleFlowFactories extends GhidraScript {
    @Override
    public void run() throws Exception {
        long[] vas = {
            0x14083a540L, 0x140839380L, 0x140839290L, 0x140838a00L,
            0x140839730L, 0x140839660L, 0x1408395a0L, 0x140839560L, 0x1408391f0L,
            0x140b0e620L // commit-into-owner (STEP_BeginLogo) for cross-check
        };
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (long v : vas) {
            Function f = getFunctionContaining(toAddr(v));
            println("==================== 0x" + Long.toHexString(v) + " ====================");
            if (f == null) { println("  NO FUNCTION"); continue; }
            println("name=" + f.getName() + " sig=" + f.getSignature());
            DecompileResults r = di.decompileFunction(f, 60, monitor);
            if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("  FAIL");
        }
    }
}
