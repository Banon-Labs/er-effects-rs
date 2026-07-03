// SoundFuncSweep: list functions whose demangled/plain name suggests sound/SE playback
// (er-effects-rs-7qp: find the menu-SE play helper the title decide/OK handlers call).
// Run: bash scripts/ghidra-query.sh scripts/ghidra/SoundFuncSweep.java [extraPatternCsv]
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.util.regex.Pattern;

public class SoundFuncSweep extends GhidraScript {
    @Override
    public void run() throws Exception {
        String extra = getScriptArgs().length > 0 ? "|" + String.join("|", getScriptArgs()[0].split(",")) : "";
        Pattern pat = Pattern.compile(
            "(?i)(sound|playse|se_?play|soundevent|wwise|\\bak[A-Z_]|menu_?se|sepost|postevent" + extra + ")");
        FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
        int shown = 0;
        while (it.hasNext() && !monitor.isCancelled()) {
            Function f = it.next();
            String name = f.getName();
            if (pat.matcher(name).find()) {
                String ns = f.getParentNamespace() != null ? f.getParentNamespace().getName(true) : "";
                println(String.format("%s  %s  ns=%s  size=%d", f.getEntryPoint(), name, ns,
                        f.getBody().getNumAddresses()));
                shown++;
                if (shown > 400) { println("(truncated at 400)"); break; }
            }
        }
        println("total matches shown: " + shown);
    }
}
