// Decompile open_menu/registrar, press-accept handler, list builder (dump VAs).
// Usage: ghidra-query.sh scripts/ghidra/OpenMenuDecomp.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class OpenMenuDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        long[] vas = {
            0x1409b2630L, // open_menu / registrar  (deobf 0x1409b24e0)
            0x1409b13b0L, // press-accept handler    (deobf 0x1409b1260)
            0x14081f270L, // list builder            (deobf 0x14081f180)
            0x140749ae0L  // set_state (FD4 SM)      (deobf set_state)
        };
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (long v : vas) {
            Address a = toAddr(v);
            Function f = getFunctionContaining(a);
            println("====================================================");
            println("VA(dump)=0x" + Long.toHexString(v));
            if (f == null) { println("  NO FUNCTION at this address"); continue; }
            println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint());
            println("  sig=" + f.getSignature());
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r != null && r.decompileCompleted()) {
                println(r.getDecompiledFunction().getC());
            } else {
                println("  DECOMPILE FAILED: " + (r==null?"null":r.getErrorMessage()));
            }
        }
    }
}
