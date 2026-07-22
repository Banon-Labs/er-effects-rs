// SymbolAddr.java <name> [<name>...]
// Reverse of NameAddrs: resolve symbol NAMES to addresses. Exact match first;
// falls back to substring matches (capped) so partial names still locate globals.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;
import ghidra.program.model.symbol.SymbolTable;

public class SymbolAddr extends GhidraScript {
    @Override
    public void run() throws Exception {
        SymbolTable st = currentProgram.getSymbolTable();
        for (String name : getScriptArgs()) {
            int hits = 0;
            SymbolIterator it = st.getSymbols(name);
            while (it.hasNext()) {
                Symbol s = it.next();
                println("EXACT " + name + " -> " + s.getAddress() + "  " + s.getName(true));
                hits++;
            }
            if (hits == 0) {
                SymbolIterator all = st.getSymbolIterator("*" + name + "*", false);
                while (all.hasNext() && hits < 25) {
                    Symbol s = all.next();
                    println("SUBSTR " + name + " -> " + s.getAddress() + "  " + s.getName(true));
                    hits++;
                }
            }
            if (hits == 0) println("(no symbol matching " + name + ")");
        }
    }
}
