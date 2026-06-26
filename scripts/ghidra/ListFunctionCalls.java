import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.symbol.Reference;

public class ListFunctionCalls extends GhidraScript {
    @Override
    public void run() throws Exception {
        for (String a : getScriptArgs()) {
            long va = Long.decode(a);
            Address addr = toAddr(va);
            Function f = getFunctionContaining(addr);
            if (f == null) {
                println("NO_FUNC " + a);
                continue;
            }
            println("FUNC " + f.getName() + " entry=" + f.getEntryPoint());
            Instruction ins = getInstructionAt(f.getEntryPoint());
            while (ins != null && f.getBody().contains(ins.getAddress())) {
                for (Reference r : ins.getReferencesFrom()) {
                    if (r.getReferenceType().isCall()) {
                        Function tf = getFunctionAt(r.getToAddress());
                        println("  CALL " + ins.getAddress() + " -> " + r.getToAddress() + (tf == null ? "" : " " + tf.getName()));
                    }
                }
                ins = ins.getNext();
            }
        }
    }
}
