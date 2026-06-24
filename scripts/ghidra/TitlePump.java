// Decompile the title update pump (dump 0x1409aad60 = deobf 0x1409aac10) to confirm it
// drains the menu-job chain; decompile FUN_1409b1050 (calls chain-entry FUN_1409a7940);
// identify the vtable that owns the chain-entry slot (DATA ref 0x144902450) by listing
// the symbol at the vtable head and the surrounding slots.
// Usage: ghidra-query.sh scripts/ghidra/TitlePump.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class TitlePump extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 220, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    // Print the vtable head symbol for a slot data-ref and a few neighbouring slot targets.
    void vtblOf(long slotRef) {
        println("---- VTABLE around slot @0x"+Long.toHexString(slotRef)+" ----");
        Memory mem = currentProgram.getMemory();
        // walk back to find a symbol (vtable head) within 0x100
        long head = -1;
        for (long off=0; off<0x100; off+=8) {
            Symbol ps = getSymbolAt(toAddr(slotRef-off));
            if (ps != null) { println("  head -0x"+Long.toHexString(off)+" = "+ps.getName()+" @0x"+Long.toHexString(slotRef-off)); head=slotRef-off; break; }
        }
        // print slots from head
        long start = head>=0? head : slotRef-0x20;
        for (int i=0;i<12;i++) {
            try {
                long p = mem.getLong(toAddr(start+(long)i*8));
                Function ff = getFunctionAt(toAddr(p));
                println("  [0x"+Long.toHexString(start+i*8)+"] -> 0x"+Long.toHexString(p)+(ff==null?"":(" "+ff.getName())));
            } catch (Exception e) {}
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        dec(0x1409aad60L, "TITLE UPDATE PUMP 0x1409aac10");
        dec(0x1409b1050L, "JOB-RUN FUN_1409b1050 (calls chain entry)");
        vtblOf(0x144902450L);
    }
}
