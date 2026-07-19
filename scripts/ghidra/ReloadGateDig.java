// ReloadGateDig.java
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.app.decompiler.*;
import java.util.*;

public class ReloadGateDig extends GhidraScript {
    DecompInterface di;
    FunctionManager fm;
    Listing lst;

    void decomp(String label, String a) throws Exception {
        Address addr = currentProgram.getAddressFactory().getAddress(a);
        Function f = fm.getFunctionContaining(addr);
        println("==================================================================");
        println("### " + label + "  arg=" + a);
        if (f == null) { println("  NO FUNCTION at " + a); return; }
        println("FUNC " + f.getName(true) + " entry=" + f.getEntryPoint()
                + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
        try {
            DecompileResults res = di.decompileFunction(f, 90, monitor);
            if (res != null && res.decompileCompleted()) {
                String c = res.getDecompiledFunction().getC();
                if (c.length() > 14000) c = c.substring(0, 14000) + "\n...[truncated]...";
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
                    callees.add((tf==null? r.getToAddress().toString() : tf.getName(true)+" @ "+tf.getEntryPoint()));
                }
            }
        }
        println("--- CALLEES (" + callees.size() + ") ---");
        for (String c : callees) println("  " + c);
    }

    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        fm = currentProgram.getFunctionManager();
        lst = currentProgram.getListing();

        decomp("STEP_Finish (MoveMapStep::STEP_Finish) dump", "0x140af5b10");
        decomp("STEP_MoveMap_Update (requestCode 1->2) dump", "0x140aec810");

        di.dispose();
        println("### DONE");
    }
}
