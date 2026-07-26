// FullDecomp.java <dumpVA> [<dumpVA> ...]
// Decompile each containing function and print the FULL C (no truncation).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.app.decompiler.*;

public class FullDecomp extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        for (String a : args) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = fm.getFunctionContaining(addr);
            println("==================================================================");
            if (f == null) { println("ARG " + a + " -> NO FUNCTION"); continue; }
            println("FUNC " + f.getName(true) + " entry=" + f.getEntryPoint()
                    + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
            try {
                DecompileResults res = di.decompileFunction(f, 90, monitor);
                if (res != null && res.decompileCompleted()) {
                    println(res.getDecompiledFunction().getC());
                } else println("  DECOMP FAILED: " + (res==null?"null":res.getErrorMessage()));
            } catch (Exception e) { println("  decompile exception: " + e); }
        }
    }
}
