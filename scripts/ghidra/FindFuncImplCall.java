// FindFuncImplCall.java <substr1> [<substr2> ...]
// Find _Func_impl vtables whose RTTI TypeDescriptor name contains the given substring,
// then decompile slot[2] (the _Do_call / operator()).
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.util.task.ConsoleTaskMonitor;

public class FindFuncImplCall extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] subs = getScriptArgs();
        SymbolTable st = currentProgram.getSymbolTable();
        FunctionManager fm = currentProgram.getFunctionManager();
        Memory mem = currentProgram.getMemory();
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);

        // Look through symbols that look like vftable for _Func_impl with our lambda substrings.
        SymbolIterator it = st.getSymbolIterator();
        while (it.hasNext()) {
            Symbol s = it.next();
            String nm = s.getName(true);
            if (!nm.contains("_Func_impl")) continue;
            boolean hit = false;
            for (String sub : subs) if (nm.contains(sub)) { hit = true; break; }
            if (!hit) continue;
            if (nm.indexOf("vftable") < 0 && nm.indexOf("vtable") < 0) continue;
            Address vt = s.getAddress();
            long slot2 = mem.getLong(vt.add(0x10));   // [2]
            Address tgt = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(slot2));
            Function f = fm.getFunctionContaining(tgt);
            println("============================================================");
            println("FUNC_IMPL vt @ " + vt + "  " + nm);
            println("  slot[2] _Do_call -> 0x"+Long.toHexString(slot2)+"  "+(f!=null?f.getName()+"@"+f.getEntryPoint():"?"));
            if (f != null) {
                DecompileResults res = dec.decompileFunction(f, 60, new ConsoleTaskMonitor());
                if (res != null && res.decompileCompleted())
                    println(res.getDecompiledFunction().getC());
            }
        }
        dec.dispose();
    }
}
