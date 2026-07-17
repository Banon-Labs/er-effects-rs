import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import java.util.List;
public class RtByName extends GhidraScript {
    @Override public void run() throws Exception {
        DecompInterface di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        for (String nm : getScriptArgs()) {
            List<Function> fs = getGlobalFunctions(nm);
            println("################ " + nm + " -> " + (fs.isEmpty()?"NOT FOUND":fs.get(0).getEntryPoint().toString()) + " ################");
            if (fs.isEmpty()) continue;
            DecompileResults r = di.decompileFunction(fs.get(0), 120, monitor);
            if (r!=null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("(fail)");
        }
    }
}
