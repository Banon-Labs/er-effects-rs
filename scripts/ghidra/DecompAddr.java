// DecompAddr: decompile the function containing each DUMP VA arg (hex 0x...). Used to understand
// boot-profiler hot loops. Run: bash scripts/ghidra-query.sh scripts/ghidra/DecompAddr.java 0x..
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class DecompAddr extends GhidraScript {
    @Override
    public void run() throws Exception {
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function f = getFunctionContaining(addr);
            if (f == null) { println("=== " + a + " NO_FUNC ==="); continue; }
            println("=== " + a + " -> " + f.getName() + " entry=" + f.getEntryPoint() + " ===");
            DecompileResults r = di.decompileFunction(f, 60, monitor);
            if (r != null && r.decompileCompleted()) {
                println(r.getDecompiledFunction().getC());
            } else {
                println("(decompile failed)");
            }
        }
    }
}
