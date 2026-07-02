// DumpMemoryFloats.java <addr> <float_count>
// Small bounded Ghidra query helper: dump float32 values from the currently-open program.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.Memory;

public class DumpMemoryFloats extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        if (a.length < 2) {
            println("usage: DumpMemoryFloats <addr> <float_count>");
            return;
        }
        Address addr = toAddr(Long.decode(a[0]));
        int count = Integer.decode(a[1]);
        Memory mem = currentProgram.getMemory();
        println("FLOATS " + addr + " count=" + count);
        for (int i = 0; i < count; i++) {
            Address cur = addr.add((long)i * 4L);
            int bits = mem.getInt(cur);
            float value = Float.intBitsToFloat(bits);
            println(String.format("[%02d] %s bits=0x%08x f=% .9g", i, cur, bits, value));
        }
    }
}
