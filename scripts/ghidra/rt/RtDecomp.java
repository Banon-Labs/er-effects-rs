import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class RtDecomp extends GhidraScript {
    @Override public void run() throws Exception {
        DecompInterface di = new DecompInterface();
        di.setOptions(new DecompileOptions());
        di.openProgram(currentProgram);
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function f = getFunctionContaining(addr);
            println("################ " + a + " -> " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "NO_FUNC") + " ################");
            if (f == null) continue;
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            if (r == null) { println("(null results)"); continue; }
            if (!r.decompileCompleted()) { println("(FAILED: " + r.getErrorMessage() + ")"); continue; }
            println(r.getDecompiledFunction().getC());
        }
    }
}
