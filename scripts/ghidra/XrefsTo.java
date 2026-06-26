import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;

public class XrefsTo extends GhidraScript {
    @Override
    public void run() throws Exception {
        for (String arg : getScriptArgs()) {
            Address a = toAddr(Long.decode(arg));
            println("=== XREFS_TO " + a + " ===");
            Reference[] refs = getReferencesTo(a);
            int n = 0;
            for (Reference r : refs) {
                Address from = r.getFromAddress();
                Function f = getFunctionContaining(from);
                println("  REF from " + from + " type=" + r.getReferenceType() + (f == null ? " NO_FUNC" : " func=" + f.getName() + " entry=" + f.getEntryPoint()));
                n++;
            }
            println("  refs=" + n);
        }
    }
}
