import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.Memory;

public class DumpAsciiPointerTable extends GhidraScript {
    private String readAscii(Address a, int max) throws Exception {
        StringBuilder sb = new StringBuilder();
        Memory mem = currentProgram.getMemory();
        for (int i = 0; i < max; i++) {
            byte b = mem.getByte(a.add(i));
            if (b == 0) break;
            if (b < 0x20 || b > 0x7e) {
                sb.append(String.format("\\x%02x", b & 0xff));
            } else {
                sb.append((char)b);
            }
        }
        return sb.toString();
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            println("usage: DumpAsciiPointerTable <address> <count> [stride=8] [ptrOffset=0]");
            return;
        }
        Address base = currentProgram.getAddressFactory().getAddress(args[0]);
        int count = Integer.decode(args[1]);
        int stride = args.length >= 3 ? Integer.decode(args[2]) : 8;
        int ptrOffset = args.length >= 4 ? Integer.decode(args[3]) : 0;
        Memory mem = currentProgram.getMemory();
        for (int i = 0; i < count; i++) {
            Address slot = base.add((long)i * stride + ptrOffset);
            long raw = mem.getLong(slot);
            Address ptr = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(raw);
            String s;
            try {
                s = readAscii(ptr, 128);
            } catch (Exception e) {
                s = "<unreadable>";
            }
            println(String.format("[%02d] slot=%s ptr=0x%016x str=%s", i, slot, raw, s));
        }
    }
}
