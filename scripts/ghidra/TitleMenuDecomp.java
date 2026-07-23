// Decompile title-menu build functions for STEP_BeginLogo analysis.
// Usage: ghidra-query.sh scripts/ghidra/TitleMenuDecomp.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class TitleMenuDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        long[] vas = {
            0x140b0c390L, // STEP_BeginLogo (dump)
            0x14081f270L, // list builder (dump)
            0x140749ae0L  // set_state (dump)
        };
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (long v : vas) {
            Address a = toAddr(v);
            Function f = getFunctionContaining(a);
            println("====================================================");
            println("VA(dump)=0x" + Long.toHexString(v));
            if (f == null) {
                println("  NO FUNCTION at this address");
                continue;
            }
            println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint());
            println("  sig=" + f.getSignature());
            DecompileResults r = di.decompileFunction(f, 60, monitor);
            if (r != null && r.decompileCompleted()) {
                DecompiledFunction df = r.getDecompiledFunction();
                println(df.getC());
            } else {
                println("  DECOMPILE FAILED: " + (r==null?"null":r.getErrorMessage()));
            }
        }
    }
}
