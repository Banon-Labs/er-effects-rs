// VtableOwner.java <addr_in_vtable_dump_va>
// Walk backward from an address inside a vtable to find the vtable start (slot whose
// preceding qword points into .rdata RTTI COL), report the slot index, and try to read
// the RTTI TypeDescriptor name. Also dump nearby slots with function names.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class VtableOwner extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Address a = currentProgram.getAddressFactory().getAddress(args[0]);
        Memory mem = currentProgram.getMemory();
        FunctionManager fm = currentProgram.getFunctionManager();
        long base = currentProgram.getImageBase().getOffset();

        // Walk backward up to 64 slots looking for a COL pointer (value whose +0xc RVA
        // resolves to a TypeDescriptor with ".?AV" name).
        Address vtStart = null; long colp = 0; String name = null;
        for (int i = 0; i < 80; i++) {
            Address cand = a.subtract((long)i * 8);          // candidate vtable[0]
            Address metaSlot = cand.subtract(8);             // COL pointer sits at vt-8
            try {
                long mp = mem.getLong(metaSlot);
                Address col = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(mp));
                int tdRva = mem.getInt(col.add(0xc));
                Address td = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(base+(tdRva&0xffffffffL)));
                StringBuilder sb = new StringBuilder();
                for (int j=0;j<200;j++){ byte b=mem.getByte(td.add(0x10+j)); if(b==0)break; sb.append((char)(b&0xff)); }
                if (sb.indexOf(".?A")>=0) { vtStart=cand; colp=mp; name=sb.toString(); break; }
            } catch (Exception e) { /* keep walking */ }
        }
        if (vtStart==null){ println("vtable start not found within 80 slots of "+a); return; }
        long slotIdx = (a.getOffset()-vtStart.getOffset())/8;
        println("VTABLE START: "+vtStart+"  COL=0x"+Long.toHexString(colp)+"  RTTI="+name);
        println("TARGET slot index = ["+slotIdx+"]");
        for (int i=0;i<14;i++){
            Address slot=vtStart.add((long)i*8);
            long p=mem.getLong(slot);
            Address tgt=currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(p));
            Function f=fm.getFunctionContaining(tgt);
            println("  ["+i+"] -> 0x"+Long.toHexString(p)+"  "+(f!=null?f.getName():"?")+(i==slotIdx?"   <== TARGET":""));
        }
    }
}
