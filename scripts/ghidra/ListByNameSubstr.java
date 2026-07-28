// ListByNameSubstr.java <substr> [<substr> ...]
// Lists every function whose name contains any given substring (case-insensitive):
// name, entry, size, signature.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import java.util.*;

public class ListByNameSubstr extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        ArrayList<String> needles = new ArrayList<>();
        for (String a : args) needles.add(a.toLowerCase());
        FunctionManager fm = currentProgram.getFunctionManager();
        FunctionIterator it = fm.getFunctions(true);
        int n = 0;
        while (it.hasNext()) {
            Function f = it.next();
            String nm = f.getName(true).toLowerCase();
            boolean hit = false;
            for (String needle : needles) if (nm.contains(needle)) { hit = true; break; }
            if (!hit) continue;
            println(f.getEntryPoint() + "  size=0x" + Long.toHexString(f.getBody().getNumAddresses())
                    + "  " + f.getName(true));
            if (++n > 400) { println("...more..."); break; }
        }
        println("TOTAL " + n);
    }
}
