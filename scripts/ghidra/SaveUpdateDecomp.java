// Decompile the save-data-update function (containing the 401112/401106 msg callers)
// and the job-runner Run() stack around it (dump VAs).
// Usage: ghidra-query.sh scripts/ghidra/SaveUpdateDecomp.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class SaveUpdateDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        long[] vas = {
            0x140826c97L, // 401106 "corrupted" caller (deobf 0x140826ba7)
            0x140826ca8L, // 401112 "updating"  caller (deobf 0x140826bb8)
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
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            if (r != null && r.decompileCompleted()) {
                println(r.getDecompiledFunction().getC());
            } else {
                println("  DECOMPILE FAILED: " + (r==null?"null":r.getErrorMessage()));
            }
        }
    }
}
