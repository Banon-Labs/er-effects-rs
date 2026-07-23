// DecompCallees.java <dumpVA> [<dumpVA> ...]
// Decompile each function + list distinct CALL callees (name @ entry).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.app.decompiler.*;
import java.util.*;

public class DecompCallees extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing lst = currentProgram.getListing();
        for (String a : args) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = fm.getFunctionContaining(addr);
            println("==================================================================");
            if (f == null) { println("ARG " + a + " -> NO FUNCTION"); continue; }
            println("FUNC " + f.getName(true) + " entry=" + f.getEntryPoint()
                    + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
            try {
                DecompileResults res = di.decompileFunction(f, 60, monitor);
                if (res != null && res.decompileCompleted()) {
                    String c = res.getDecompiledFunction().getC();
                    if (c.length() > 8000) c = c.substring(0, 8000) + "\n...[truncated]...";
                    println(c);
                } else println("  DECOMP FAILED: " + (res==null?"null":res.getErrorMessage()));
            } catch (Exception e) { println("  decompile exception: " + e); }
            LinkedHashSet<String> callees = new LinkedHashSet<>();
            InstructionIterator ii = lst.getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction insn = ii.next();
                if (!insn.getMnemonicString().equalsIgnoreCase("call")) continue;
                for (Reference r : insn.getReferencesFrom()) {
                    if (r.getReferenceType().isCall()) {
                        Function tf = fm.getFunctionContaining(r.getToAddress());
                        callees.add(tf==null? r.getToAddress().toString() : tf.getName(true)+" @ "+tf.getEntryPoint());
                    }
                }
            }
            println("--- CALLEES ---");
            for (String c : callees) println("  " + c);
        }
        di.dispose();
    }
}
