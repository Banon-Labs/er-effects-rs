// XrefRtti.java
// For each arg: if "xref:0xADDR" print callers (functions referencing it) + the call instruction.
//               if "rtti:0xADDR" read a TypeDescriptor/RTTI-col pointer and try to print the mangled name string.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;

public class XrefRtti extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        FunctionManager fm = currentProgram.getFunctionManager();
        ReferenceManager rm = currentProgram.getReferenceManager();
        Memory mem = currentProgram.getMemory();
        for (String spec : args) {
            String hex = spec.substring(spec.indexOf(':')+1);
            Address a = currentProgram.getAddressFactory().getAddress(hex);
            if (spec.startsWith("xref:")) {
                println("=== XREFS to " + a + " ===");
                ReferenceIterator it = rm.getReferencesTo(a);
                int cnt = 0;
                while (it.hasNext() && cnt < 60) {
                    Reference r = it.next();
                    Address from = r.getFromAddress();
                    Function cf = fm.getFunctionContaining(from);
                    println("  from " + from + " (" + (cf!=null?cf.getName()+"@"+cf.getEntryPoint():"?") + ")  type=" + r.getReferenceType());
                    cnt++;
                }
                if (cnt == 0) println("  (none)");
            } else if (spec.startsWith("rtti:")) {
                // RTTICompleteObjectLocator: +0xC = TypeDescriptor RVA (image-relative). Read several candidate layouts.
                println("=== RTTI col @ " + a + " ===");
                try {
                    long base = currentProgram.getImageBase().getOffset();
                    int sig = mem.getInt(a);
                    int tdRva = mem.getInt(a.add(0xc));
                    println("  signature=" + sig + " tdRVA=0x" + Integer.toHexString(tdRva));
                    Address td = currentProgram.getAddressFactory().getAddress("0x" + Long.toHexString(base + (tdRva & 0xffffffffL)));
                    // TypeDescriptor: +0x10 = name (char[]) for MSVC
                    Address nameAddr = td.add(0x10);
                    StringBuilder sb = new StringBuilder();
                    for (int i = 0; i < 200; i++) {
                        byte b = mem.getByte(nameAddr.add(i));
                        if (b == 0) break;
                        sb.append((char)(b & 0xff));
                    }
                    println("  TypeDescriptor name: " + sb);
                } catch (Exception e) { println("  parse fail: " + e); }
            }
        }
    }
}
