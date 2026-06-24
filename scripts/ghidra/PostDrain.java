// Resolve WHERE open_menu's job chain is installed (FUN_14078e1d0 / FUN_1407928c0 final post)
// and whether the pump's drain FUN_140745670 (MenuWindow::Update) ticks that same object/offset.
// Usage: ghidra-query.sh scripts/ghidra/PostDrain.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class PostDrain extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 200, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        // open_menu chain post tail:
        dec(0x14078e1d0L, "POST A FUN_14078e1d0");
        dec(0x1407928c0L, "POST B FUN_1407928c0");
        dec(0x14078c620L, "INSERT FUN_14078c620 (FUN_1409aa580's installer)");
        // pump drain:
        dec(0x140745670L, "PUMP DRAIN FUN_140745670 (MenuWindow::Update)");
        dec(0x1409b3f20L, "PUMP TAIL FUN_1409b3f20");
    }
}
