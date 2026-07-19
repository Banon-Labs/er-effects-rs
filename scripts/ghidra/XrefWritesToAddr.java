// XrefWritesToAddr.java 0xADDR [0xADDR...]
// Print only WRITE/READ_WRITE references to each target address, including containing function.
// Use for singleton/global pointer ownership without scrolling through hundreds of read refs.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import ghidra.program.model.symbol.ReferenceManager;

public class XrefWritesToAddr extends GhidraScript {
    @Override
    public void run() throws Exception {
        ReferenceManager rm = currentProgram.getReferenceManager();
        FunctionManager fm = currentProgram.getFunctionManager();
        for (String s : getScriptArgs()) {
            Address addr = currentProgram.getAddressFactory().getAddress(s);
            println("target " + addr);
            ReferenceIterator it = rm.getReferencesTo(addr);
            int printed = 0;
            int seen = 0;
            while (it.hasNext()) {
                Reference r = it.next();
                seen++;
                if (!r.getReferenceType().isWrite()) {
                    continue;
                }
                Function f = fm.getFunctionContaining(r.getFromAddress());
                println("  write " + r.getFromAddress() + " " + r.getReferenceType()
                        + (f == null ? "  (no func)" : "  in " + f.getName() + " entry=" + f.getEntryPoint()));
                printed++;
            }
            println("  writes=" + printed + " refs_seen=" + seen);
        }
    }
}
