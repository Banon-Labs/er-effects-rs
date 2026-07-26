import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.*;

// RtSyms <kw1> <kw2> ... : list named symbols whose (lowercased) name contains any keyword.
public class RtSyms extends GhidraScript {
    @Override public void run() throws Exception {
        String[] kws = getScriptArgs();
        SymbolIterator si = currentProgram.getSymbolTable().getAllSymbols(true);
        int shown = 0;
        while (si.hasNext() && shown < 400) {
            Symbol s = si.next();
            String n = s.getName().toLowerCase();
            for (String k : kws) {
                if (n.contains(k.toLowerCase())) {
                    println("  " + s.getAddress() + "  " + s.getSymbolType() + "  " + s.getName());
                    shown++;
                    break;
                }
            }
        }
        println("(shown " + shown + ")");
    }
}
