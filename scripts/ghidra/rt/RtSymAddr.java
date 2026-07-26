import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.Address;
public class RtSymAddr extends GhidraScript {
    @Override public void run() throws Exception {
        SymbolTable st = currentProgram.getSymbolTable();
        for (String nm : getScriptArgs()) {
            SymbolIterator it = st.getSymbols(nm);
            boolean any=false;
            while (it.hasNext()) { Symbol s=it.next(); println(nm + " -> " + s.getAddress() + " (" + s.getSymbolType() + ", ns=" + s.getParentNamespace().getName() + ")"); any=true; }
            if(!any) println(nm + " -> (none)");
        }
    }
}
