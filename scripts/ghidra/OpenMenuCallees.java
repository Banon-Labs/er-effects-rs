// Decompile open_menu's MenuJob-constructing callees to see if any builds Continue/Load/NewGame.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class OpenMenuCallees extends GhidraScript {
    @Override
    public void run() throws Exception {
        long[] vas = {
            0x1409a6dc0L, // FUN_1409a6dc0
            0x1409ac770L, // FUN_1409ac770
            0x140833970L, // FUN_140833970
            0x1409aa420L, // FUN_1409aa420
            0x1409aa580L  // FUN_1409aa580
        };
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (long v : vas) {
            Function f = getFunctionContaining(toAddr(v));
            println("==================== 0x" + Long.toHexString(v) + " ====================");
            if (f == null) { println("  NO FUNCTION"); continue; }
            println("name=" + f.getName() + " sig=" + f.getSignature());
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("  FAIL");
        }
    }
}
