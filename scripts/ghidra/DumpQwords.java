// DumpQwords.java <VA> <count>
// Dump <count> qwords starting at VA, resolving each to a function name if any.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;

public class DumpQwords extends GhidraScript {
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address base = currentProgram.getAddressFactory().getAddress(a[0]);
        int n = Integer.decode(a[1]);
        FunctionManager fm = currentProgram.getFunctionManager();
        Memory mem = currentProgram.getMemory();
        for (int i = 0; i < n; i++) {
            Address slot = base.add((long)i*8);
            try {
                long v = mem.getLong(slot);
                Address va = currentProgram.getAddressFactory().getAddress(Long.toHexString(v));
                Function tf = fm.getFunctionContaining(va);
                String nm = tf==null? "" : ("  " + tf.getName(true) + (tf.getEntryPoint().equals(va)?" [ENTRY]":" +0x"+Long.toHexString(v-tf.getEntryPoint().getOffset())));
                println("  +0x"+Long.toHexString(i*8)+" "+slot+": 0x"+Long.toHexString(v)+nm);
            } catch (Exception e) { println("  +0x"+Long.toHexString(i*8)+" ERR "+e.getMessage()); }
        }
    }
}
