// InputMgrProbe.java  -- one-shot inputmgr struct/field/class probe (dump VAs)
// Decompiles the producer/writer/helper/predicates and inspects the global @ 0x143d6b7b0.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.data.*;
import ghidra.app.decompiler.*;
import java.util.*;

public class InputMgrProbe extends GhidraScript {
    DecompInterface di;
    FunctionManager fm;

    void decomp(String label, String va, int max) {
        Address addr = currentProgram.getAddressFactory().getAddress(va);
        Function f = fm.getFunctionContaining(addr);
        println("==================================================================");
        println("### " + label + "  dumpVA=" + va);
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  FUNC " + f.getName(true) + " entry=" + f.getEntryPoint()
                + " size=0x" + Long.toHexString(f.getBody().getNumAddresses()));
        try {
            DecompileResults res = di.decompileFunction(f, 60, monitor);
            if (res != null && res.decompileCompleted()) {
                String c = res.getDecompiledFunction().getC();
                if (c.length() > max) c = c.substring(0, max) + "\n...[truncated]...";
                println(c);
            } else println("  DECOMP FAILED: " + (res==null?"null":res.getErrorMessage()));
        } catch (Exception e) { println("  decompile exception: " + e); }
    }

    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        fm = currentProgram.getFunctionManager();

        // --- The global singleton pointer @ 0x143d6b7b0 ---
        Address g = currentProgram.getAddressFactory().getAddress("0x143d6b7b0");
        println("### GLOBAL @ 0x143d6b7b0");
        SymbolTable st = currentProgram.getSymbolTable();
        Symbol[] syms = st.getSymbols(g);
        for (Symbol s : syms) println("  sym: " + s.getName() + " (" + s.getSymbolType() + ")");
        Data d = currentProgram.getListing().getDataAt(g);
        if (d != null) println("  dataType: " + d.getDataType().getName() + "  value=" + d.getDefaultValueRepresentation());
        println("  --- xrefs TO global (functions touching it) ---");
        ReferenceIterator ri = currentProgram.getReferenceManager().getReferencesTo(g);
        int nx=0;
        while (ri.hasNext() && nx<40) {
            Reference r = ri.next();
            Function tf = fm.getFunctionContaining(r.getFromAddress());
            println("    " + r.getFromAddress() + " " + r.getReferenceType()
                + "  in " + (tf==null?"?":tf.getName()+" @ "+tf.getEntryPoint()));
            nx++;
        }

        decomp("PRODUCER (leaf input writer, deobf 0x1407ad1c0)", "0x1407ad2b0", 7000);
        decomp("EVENT-ARRAY WRITER (deobf 0x140766340)", "0x140766430", 4000);
        decomp("ID-STORE/ACTION-NAMER (deobf 0x140767df0)", "0x140767ee0", 2000);
        decomp("CONFIRM PREDICATE (deobf 0x140765d40)", "0x140765e30", 4000);
        decomp("VERTICAL PREDICATE (deobf 0x140765780)", "0x140765870", 4000);
        di.dispose();
    }
}
