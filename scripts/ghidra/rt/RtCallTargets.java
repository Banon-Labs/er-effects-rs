import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
public class RtCallTargets extends GhidraScript {
    @Override public void run() throws Exception {
        // args: <funcContainingVA> -> list all CALL targets with resolved names
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function f = getFunctionContaining(addr);
            println("################ CALLs inside " + (f!=null?f.getName():"?") + " @ " + (f!=null?f.getEntryPoint():"?") + " ################");
            if (f==null) continue;
            InstructionIterator it = currentProgram.getListing().getInstructions(f.getBody(), true);
            while (it.hasNext()) {
                Instruction ins = it.next();
                String m = ins.getMnemonicString();
                if (m.startsWith("CALL")) {
                    Reference[] refs = ins.getReferencesFrom();
                    for (Reference r : refs) {
                        if (r.getReferenceType().isCall()) {
                            Function tf = getFunctionContaining(r.getToAddress());
                            println("  " + ins.getAddress() + "  CALL " + r.getToAddress() + "  " + (tf!=null?tf.getName():"?"));
                        }
                    }
                }
            }
        }
    }
}
