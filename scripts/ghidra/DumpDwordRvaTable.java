// DumpDwordRvaTable.java <startVA> <count>
// Reads <count> 4-byte little-endian values at startVA; interprets each as an RVA
// (adds image base) and resolves to the containing function name. Prints index/value/name.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;

public class DumpDwordRvaTable extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Address start = currentProgram.getAddressFactory().getAddress(args[0]);
        int count = Integer.decode(args[1]);
        long base = currentProgram.getImageBase().getOffset();
        FunctionManager fm = currentProgram.getFunctionManager();
        Memory mem = currentProgram.getMemory();
        for (int i = 0; i < count; i++) {
            Address slot = start.add((long)i*4);
            int v = mem.getInt(slot);
            long va = base + (v & 0xffffffffL);
            Address a = currentProgram.getAddressFactory().getAddress(Long.toHexString(va));
            Function tf = fm.getFunctionContaining(a);
            String nm = tf==null? "(no func)" : (tf.getName(true) + (tf.getEntryPoint().equals(a)?" [ENTRY]":" +0x"+Long.toHexString(va-tf.getEntryPoint().getOffset())));
            println("  [" + i + "] " + slot + ": rva=0x" + Integer.toHexString(v) + " -> 0x" + Long.toHexString(va) + "  " + nm);
        }
    }
}
