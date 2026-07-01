// TitleFlowDecomp.java
// Decompile a set of functions (by symbol name or by address) and print callees.
// Usage: ghidra-query.sh scripts/ghidra/TitleFlowDecomp.java <spec> [<spec> ...]
//   <spec> = "name:STEP_BeginLogo"  or  "addr:0x140b0c2a0"
// Prints: function entry, signature, decompiled C, and list of called functions w/ addrs.

import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.util.task.ConsoleTaskMonitor;
import java.util.*;

public class TitleFlowDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();

        for (String spec : args) {
            Function f = null;
            if (spec.startsWith("name:")) {
                String nm = spec.substring(5);
                for (Symbol s : st.getSymbols(nm)) {
                    Address a = s.getAddress();
                    Function ff = fm.getFunctionAt(a);
                    if (ff == null) ff = fm.getFunctionContaining(a);
                    if (ff != null) { f = ff; break; }
                }
                if (f == null) {
                    // substring search
                    SymbolIterator it = st.getSymbolIterator();
                    while (it.hasNext()) {
                        Symbol s = it.next();
                        if (s.getName().contains(nm)) {
                            Function ff = fm.getFunctionContaining(s.getAddress());
                            if (ff != null) { f = ff; println("  (matched by substring: " + s.getName() + ")"); break; }
                        }
                    }
                }
            } else if (spec.startsWith("addr:")) {
                Address a = currentProgram.getAddressFactory().getAddress(spec.substring(5));
                f = fm.getFunctionAt(a);
                if (f == null) f = fm.getFunctionContaining(a);
            }

            println("============================================================");
            println("SPEC: " + spec);
            if (f == null) { println("  NOT FOUND"); continue; }
            println("FUNC: " + f.getName() + " @ " + f.getEntryPoint());
            println("SIG : " + f.getPrototypeString(true, false));

            DecompileResults res = dec.decompileFunction(f, 60, new ConsoleTaskMonitor());
            if (res != null && res.decompileCompleted()) {
                println("----- DECOMP -----");
                println(res.getDecompiledFunction().getC());
            } else {
                println("  DECOMP FAILED: " + (res==null?"null":res.getErrorMessage()));
            }

            println("----- CALLEES -----");
            Set<Function> callees = f.getCalledFunctions(new ConsoleTaskMonitor());
            TreeMap<String,String> sorted = new TreeMap<>();
            for (Function c : callees) sorted.put(c.getEntryPoint().toString(), c.getName());
            for (Map.Entry<String,String> e : sorted.entrySet())
                println("  " + e.getKey() + "  " + e.getValue());
        }
        dec.dispose();
    }
}
