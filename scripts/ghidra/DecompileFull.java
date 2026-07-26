// DecompileFull.java <dumpVA> [timeoutSeconds]
// Prints the full decompiler C output for the function containing <dumpVA>.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class DecompileFull extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 1) {
            println("usage: DecompileFull.java <dumpVA> [timeoutSeconds]");
            return;
        }
        int timeout = args.length >= 2 ? Integer.parseInt(args[1]) : 120;
        Address addr = toAddr(Long.decode(args[0]));
        Function f = currentProgram.getFunctionManager().getFunctionContaining(addr);
        if (f == null) {
            println("NO_FUNCTION " + addr);
            return;
        }
        println("--- FUNCTION " + f.getName(true) + " @ " + f.getEntryPoint() + " ---");
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        DecompileResults res = di.decompileFunction(f, timeout, monitor);
        if (!res.decompileCompleted()) {
            println("DECOMP_FAILED " + res.getErrorMessage());
        } else {
            println(res.getDecompiledFunction().getC());
        }
        di.dispose();
    }
}
