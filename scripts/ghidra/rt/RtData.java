import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.MemoryAccessException;

public class RtData extends GhidraScript {
    @Override public void run() throws Exception {
        // args: <hexAddr> <numQwords>
        String[] a = getScriptArgs();
        Address base = toAddr(Long.decode(a[0]));
        int n = a.length>1 ? Integer.decode(a[1]) : 16;
        println("################ qwords at " + base + " ################");
        for (int i=0;i<n;i++) {
            Address at = base.add((long)i*8);
            try {
                long v = getLong(at);
                Address tgt = toAddr(v & 0xffffffffffffL);
                String sym = "";
                try { var f = getFunctionContaining(toAddr(v)); if(f!=null) sym = f.getName()+"+" + (v - f.getEntryPoint().getOffset()); } catch(Exception e){}
                println(String.format("  +0x%03x  %016x  %s", i*8, v, sym));
            } catch(MemoryAccessException e) {
                println(String.format("  +0x%03x  <no mem>", i*8));
            }
        }
    }
}
