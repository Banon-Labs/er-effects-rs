// VtableDump.java
// Dump pointers at a vtable address and resolve each to a function name.
// Also print RTTI-ish symbol at vtable-8 (type descriptor pointer) if present.
// Usage: ghidra-query.sh scripts/ghidra/VtableDump.java <vtable_dump_va> <count>
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class VtableDump extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Address vt = currentProgram.getAddressFactory().getAddress(args[0]);
        int n = Integer.parseInt(args[1]);
        FunctionManager fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();

        // try RTTI col pointer one slot above vtable (vt-8)
        try {
            Address meta = vt.subtract(8);
            long mp = currentProgram.getMemory().getLong(meta);
            println("meta@(vt-8)=0x" + Long.toHexString(mp));
        } catch (Exception e) { println("meta read fail: " + e); }

        for (int i = 0; i < n; i++) {
            Address slot = vt.add((long)i * 8);
            long p;
            try { p = currentProgram.getMemory().getLong(slot); }
            catch (Exception e) { println("[" + i + "] read fail"); continue; }
            Address tgt = currentProgram.getAddressFactory().getAddress("0x" + Long.toHexString(p));
            Function f = fm.getFunctionAt(tgt);
            if (f == null) f = fm.getFunctionContaining(tgt);
            String nm = f != null ? f.getName() : "?";
            // any symbol at tgt
            Symbol s = st.getPrimarySymbol(tgt);
            String sym = s != null ? s.getName() : "";
            println("[" + i + "] slot=0x" + slot + " -> 0x" + Long.toHexString(p) + "  " + nm + (sym.isEmpty()?"":("  ("+sym+")")));
        }
    }
}
