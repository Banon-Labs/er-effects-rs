// XrefsToAddr.java 0xADDR [0xADDR...]
// For each data/code address: print any symbol at it, then every reference to it
// (from-address, ref type, containing function name+entry). For finding singleton
// global writers/readers (e.g. GameDataMan ctor writing the global slot).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.symbol.*;

public class XrefsToAddr extends GhidraScript {
    @Override
    public void run() throws Exception {
        ReferenceManager rm = currentProgram.getReferenceManager();
        FunctionManager fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            println("=== target " + addr + " ===");
            for (Symbol s : st.getSymbols(addr)) println("  symbol: " + s.getName(true));
            ReferenceIterator it = rm.getReferencesTo(addr);
            int n = 0;
            while (it.hasNext() && n < 400) {
                Reference r = it.next();
                Function f = fm.getFunctionContaining(r.getFromAddress());
                println("  ref " + r.getFromAddress() + " " + r.getReferenceType()
                        + (f == null ? "  (no func)" : "  in " + f.getName() + " entry=" + f.getEntryPoint()));
                n++;
            }
            println("  total_printed=" + n);
        }
    }
}
