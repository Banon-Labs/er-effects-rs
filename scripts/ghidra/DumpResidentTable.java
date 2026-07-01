// DumpResidentTable.java
// Walk a table of {qword jobResult; wchar_t* filename} entries (stride 0x10).
// args: 0xTABLE count stride filenameOffset
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.*;

public class DumpResidentTable extends GhidraScript {
    @Override
    public void run() throws Exception {
        Memory mem = currentProgram.getMemory();
        String[] a = getScriptArgs();
        long tbl = Long.decode(a[0]);
        int count = Integer.decode(a[1]);
        int stride = a.length > 2 ? Integer.decode(a[2]) : 0x10;
        int foff = a.length > 3 ? Integer.decode(a[3]) : 8;
        for (int i = 0; i < count; i++) {
            long ea = tbl + (long)i * stride;
            Address entry = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(ea));
            long jr;
            long fp;
            try {
                jr = mem.getLong(entry);
                fp = mem.getLong(entry.add(foff));
            } catch (Exception e) { println(i + ": read fail " + e); break; }
            String name = "(null)";
            if (fp != 0) {
                try {
                    Address fa = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(fp));
                    StringBuilder sb = new StringBuilder();
                    for (int j = 0; j < 128; j++) {
                        short w = mem.getShort(fa.add(j*2));
                        if (w == 0) break;
                        sb.append((char)(w & 0xffff));
                    }
                    name = sb.toString();
                } catch (Exception e) { name = "(badptr 0x"+Long.toHexString(fp)+")"; }
            }
            println(String.format("idx %3d @ %x : jobResult=0x%x filename=0x%x \"%s\"", i, ea, jr, fp, name));
        }
    }
}
