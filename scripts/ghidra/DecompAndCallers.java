// DecompAndCallers.java
// For each dump VA arg: print containing function name+entry, decompiled C, and
// the list of CALLERS (xrefs-to the function entry) with each caller's func name+entry.
// Usage: ghidra-query.sh scripts/ghidra/DecompAndCallers.java 0x140bb8e80 [0x... ...]
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import ghidra.app.decompiler.*;

public class DecompAndCallers extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        for (String a : args) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = getFunctionContaining(addr);
            println("==================================================================");
            if (f == null) {
                println("ARG " + a + " -> NO FUNCTION CONTAINING");
                // still list direct refs to the address
                listCallers(addr, a);
                continue;
            }
            println("ARG " + a + " -> FUNC " + f.getName() + " @ " + f.getEntryPoint());
            println("  sig: " + f.getSignature().getPrototypeString());
            // decompile
            try {
                DecompileResults res = di.decompileFunction(f, 60, monitor);
                if (res != null && res.decompileCompleted()) {
                    String c = res.getDecompiledFunction().getC();
                    // truncate
                    if (c.length() > 6000) c = c.substring(0, 6000) + "\n...[truncated]...";
                    println("--- DECOMP ---");
                    println(c);
                } else {
                    println("  (decompile failed)");
                }
            } catch (Exception e) {
                println("  decompile exception: " + e);
            }
            println("--- CALLERS (xrefs to entry " + f.getEntryPoint() + ") ---");
            listCallers(f.getEntryPoint(), a);
        }
        di.dispose();
    }

    void listCallers(Address entry, String label) {
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(entry);
        int n = 0;
        while (it.hasNext()) {
            Reference r = it.next();
            Address from = r.getFromAddress();
            Function cf = getFunctionContaining(from);
            String cn = cf == null ? "(none)" : (cf.getName() + " @ " + cf.getEntryPoint());
            println("  <- " + r.getReferenceType() + " from " + from + "  in " + cn);
            n++;
            if (n > 60) { println("  ...more..."); break; }
        }
        if (n == 0) println("  (no xrefs)");
    }
}
