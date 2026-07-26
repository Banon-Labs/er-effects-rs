// Decompile the function CONTAINING each address passed as a script arg, and print its C.
// Usage (choza machine): see scripts/ghidra-query-choza.sh
//   args are hex VAs in the DUMP address space (deobf VA + region shift; e.g. deobf+0x20 for 0x140e-0x141e).
// getFunctionContaining snaps to the enclosing function even if the arg is mid-function.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class DecompAt extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (String a : args) {
            long va = Long.decode(a.trim());
            Address addr = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(va);
            println("======================== arg " + a + " -> " + addr);
            Function f = getFunctionContaining(addr);
            if (f == null) {
                println("  NO FUNCTION containing " + addr);
                continue;
            }
            println("  FUNC " + f.getName() + " entry=" + f.getEntryPoint()
                    + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
            DecompileResults res = di.decompileFunction(f, 90, monitor);
            if (res != null && res.decompileCompleted()) {
                println(res.getDecompiledFunction().getC());
            } else {
                println("  DECOMPILE FAILED: " + (res != null ? res.getErrorMessage() : "null"));
            }
        }
        di.dispose();
    }
}
