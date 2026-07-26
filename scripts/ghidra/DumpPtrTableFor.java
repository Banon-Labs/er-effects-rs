// DumpPtrTableFor.java <funcVA>
// Finds DATA references to funcVA (a pointer to it stored in a table), then for each,
// walks backward/forward reading qwords and resolving each to a function name, to
// reconstruct the step-function-pointer table. Dumps 20 entries before/after each hit.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class DumpPtrTableFor extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Address target = currentProgram.getAddressFactory().getAddress(args[0]);
        FunctionManager fm = currentProgram.getFunctionManager();
        Memory mem = currentProgram.getMemory();
        ReferenceManager rm = currentProgram.getReferenceManager();
        ReferenceIterator it = rm.getReferencesTo(target);
        int hits = 0;
        while (it.hasNext()) {
            Reference r = it.next();
            if (!r.getReferenceType().isData()) continue;
            Address from = r.getFromAddress();
            println("=== DATA ref to " + target + " at " + from + " (" + r.getReferenceType() + ") ===");
            Address start = from.subtract(20*8);
            for (int i = 0; i < 40; i++) {
                Address slot = start.add((long)i*8);
                try {
                    long v = mem.getLong(slot);
                    Address va = currentProgram.getAddressFactory().getAddress(Long.toHexString(v));
                    Function tf = fm.getFunctionContaining(va);
                    String nm = tf==null? "" : ("  " + tf.getName(true) + (tf.getEntryPoint().equals(va)?" [ENTRY]":" +off"));
                    String mark = slot.equals(from) ? "  <==TARGET" : "";
                    if (tf != null || slot.equals(from))
                        println("  [" + i + "] " + slot + ": 0x" + Long.toHexString(v) + nm + mark);
                } catch (Exception e) {}
            }
            if (++hits > 6) break;
        }
        if (hits == 0) println("no data refs");
    }
}
