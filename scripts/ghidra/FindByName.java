// FindByName.java <substr> [<substr>...]
// Print functions whose name contains any substring, with entry addr.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class FindByName extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] subs = getScriptArgs();
        FunctionManager fm = currentProgram.getFunctionManager();
        for (Function f : fm.getFunctions(true)) {
            String n = f.getName();
            for (String s : subs) if (n.contains(s)) { println(f.getEntryPoint() + "  " + n); break; }
        }
    }
}
