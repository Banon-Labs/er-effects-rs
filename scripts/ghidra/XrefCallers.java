// XrefCallers: for each DUMP VA arg, list direct callers of the containing function (caller
// function name + entry), to classify a hot function's role (asset-load vs anti-tamper/DRM).
// Run: bash scripts/ghidra-query.sh scripts/ghidra/XrefCallers.java 0x.. 0x..
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;

public class XrefCallers extends GhidraScript {
    @Override
    public void run() throws Exception {
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function f = getFunctionContaining(addr);
            Address entry = (f != null) ? f.getEntryPoint() : addr;
            String fname = (f != null) ? f.getName() : a;
            println("=== callers of " + fname + " @ " + entry + " ===");
            ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(entry);
            int n = 0;
            while (it.hasNext() && n < 80) {
                Reference r = it.next();
                Address from = r.getFromAddress();
                Function cf = getFunctionContaining(from);
                String cn = (cf != null) ? cf.getName() + " @ " + cf.getEntryPoint() : "(no-func)";
                println("  <- " + from + " in " + cn + " [" + r.getReferenceType() + "]");
                n++;
            }
            println("  (" + n + " refs)");
        }
    }
}
