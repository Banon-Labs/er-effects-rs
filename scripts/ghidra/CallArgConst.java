// CallArgConst.java <0xFUNC> [0xFUNC ...]
// For each call site of the target function(s), walk back up to 12 instructions in the
// calling function and report immediate constants loaded into argument registers
// (ECX/EDX/R8D/R9D families). Used to find which stateInfo/id constants callers pass
// to generic query helpers like HasSpEffectWithStateInfo.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.*;

public class CallArgConst extends GhidraScript {
    private static final int BACKTRACK_LIMIT = 12;

    @Override
    public void run() throws Exception {
        FunctionManager fm = currentProgram.getFunctionManager();
        ReferenceManager rm = currentProgram.getReferenceManager();
        Listing lst = currentProgram.getListing();
        for (String a : getScriptArgs()) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = fm.getFunctionContaining(addr);
            Address body = (f != null) ? f.getEntryPoint() : addr;
            println("==== target " + a + " -> " + (f != null ? f.getName() : "?") + " @ " + body + " ====");
            ReferenceIterator it = rm.getReferencesTo(body);
            while (it.hasNext()) {
                Reference r = it.next();
                if (!r.getReferenceType().isCall()) continue;
                Address site = r.getFromAddress();
                Function cf = fm.getFunctionContaining(site);
                StringBuilder consts = new StringBuilder();
                Instruction ins = lst.getInstructionAt(site);
                int steps = 0;
                while (ins != null && steps < BACKTRACK_LIMIT) {
                    ins = ins.getPrevious();
                    steps++;
                    if (ins == null) break;
                    if (cf != null && !cf.getBody().contains(ins.getAddress())) break;
                    String s = ins.toString();
                    // arg-register immediate loads, e.g. "MOV EDX,0x1de"
                    if (s.startsWith("MOV ")
                            && (s.contains("ECX,0x") || s.contains("EDX,0x") || s.contains("R8D,0x")
                                || s.contains("R9D,0x") || s.contains("CX,0x") || s.contains("DX,0x")
                                || s.contains("R8W,0x") || s.contains("R15W,0x") || s.contains("R15D,0x"))) {
                        consts.append(" [").append(s).append("]");
                    }
                    if (s.startsWith("CALL")) break;
                }
                println("  site " + site + " in " + (cf != null ? cf.getName() + "@" + cf.getEntryPoint() : "?")
                        + consts);
            }
        }
    }
}
