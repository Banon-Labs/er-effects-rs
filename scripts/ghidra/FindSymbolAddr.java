// FindSymbolAddr.java <substr> [<substr> ...]
// Prints address of every symbol (data label, global, function) whose name contains a substring.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.*;

public class FindSymbolAddr extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        SymbolTable st = currentProgram.getSymbolTable();
        SymbolIterator it = st.getAllSymbols(true);
        int n = 0;
        while (it.hasNext()) {
            Symbol s = it.next();
            String nm = s.getName();
            boolean hit = false;
            for (String a : args) if (nm.toLowerCase().contains(a.toLowerCase())) { hit = true; break; }
            if (!hit) continue;
            println("  " + s.getAddress() + "  " + s.getSymbolType() + "  " + s.getName(true));
            if (++n > 120) { println("...more..."); break; }
        }
        println("TOTAL " + n);
    }
}
