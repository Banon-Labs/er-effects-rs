// DisasmCallsDecomp.java <dumpVA> [<dumpVA> ...]
// For each address: resolve containing function, print entry/size, full disassembly
// with resolved CALL targets (name @ addr), list distinct callee functions, list callers,
// and attempt a decompile (reporting failure reason if any).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.app.decompiler.*;
import ghidra.program.model.mem.*;
import java.util.*;

public class DisasmCallsDecomp extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        Listing lst = currentProgram.getListing();
        FunctionManager fm = currentProgram.getFunctionManager();
        for (String a : args) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = fm.getFunctionContaining(addr);
            println("==================================================================");
            if (f == null) { println("ARG " + a + " -> NO FUNCTION"); continue; }
            println("ARG " + a + " -> FUNC " + f.getName() + " entry=" + f.getEntryPoint()
                    + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
            println("  sig: " + f.getSignature().getPrototypeString());
            // decompile attempt
            try {
                DecompileResults res = di.decompileFunction(f, 60, monitor);
                if (res != null && res.decompileCompleted()) {
                    String c = res.getDecompiledFunction().getC();
                    if (c.length() > 9000) c = c.substring(0, 9000) + "\n...[truncated]...";
                    println("--- DECOMP ---");
                    println(c);
                } else {
                    println("--- DECOMP FAILED: " + (res == null ? "null" : res.getErrorMessage()));
                }
            } catch (Exception e) { println("  decompile exception: " + e); }
            // disassembly with call resolution
            println("--- DISASM (calls resolved) ---");
            LinkedHashSet<String> callees = new LinkedHashSet<>();
            InstructionIterator ii = lst.getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction insn = ii.next();
                String m = insn.getMnemonicString().toLowerCase();
                String extra = "";
                if (m.equals("call") || m.startsWith("j")) {
                    Reference[] rs = insn.getReferencesFrom();
                    for (Reference r : rs) {
                        if (r.getReferenceType().isCall() || r.getReferenceType().isJump()) {
                            Address t = r.getToAddress();
                            Function tf = fm.getFunctionContaining(t);
                            String nm = tf == null ? t.toString() : (tf.getName() + " @ " + tf.getEntryPoint());
                            extra = "   -> " + nm;
                            if (m.equals("call") && tf != null) callees.add(tf.getName() + " @ " + tf.getEntryPoint());
                        }
                    }
                }
                // flag writes to interesting field offsets
                String txt = insn.toString();
                println("  " + insn.getAddress() + ": " + txt + extra);
            }
            println("--- DISTINCT CALLEES ---");
            for (String c : callees) println("  " + c);
            println("--- CALLERS (xrefs to entry) ---");
            ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
            int n = 0;
            while (it.hasNext()) {
                Reference r = it.next();
                if (!r.getReferenceType().isCall() && !r.getReferenceType().isJump() && !r.getReferenceType().isData()) continue;
                Function cf = fm.getFunctionContaining(r.getFromAddress());
                println("  <- " + r.getReferenceType() + " from " + r.getFromAddress()
                        + "  in " + (cf == null ? "(none)" : cf.getName() + " @ " + cf.getEntryPoint()));
                if (++n > 40) { println("  ...more..."); break; }
            }
            if (n == 0) println("  (no xrefs)");
        }
        di.dispose();
    }
}
