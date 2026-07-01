// DecompAndXref.java
// Multi-purpose: for each arg spec, perform an action.
//   d:0xADDR        -> decompile function containing ADDR (dump VA)
//   x:0xADDR        -> list xrefs (callers/readers/writers) to ADDR with ref type + containing func
//   xd:0xADDR       -> list xrefs AND decompile each distinct containing function
//   s:0xADDR:N      -> scan N bytes at ADDR, for each address that has a reference, print refs
//   rtti:0xVT       -> read RTTI TypeDescriptor name for a vtable at VT (vt-8 COL)
//   m:0xADDR        -> print qword at ADDR (memory read)
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.util.task.ConsoleTaskMonitor;
import java.util.*;

public class DecompAndXref extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;
    ReferenceManager rm;
    Memory mem;
    long base;

    String decompile(Function f) {
        try {
            DecompileResults res = dec.decompileFunction(f, 90, new ConsoleTaskMonitor());
            if (res != null && res.decompileCompleted())
                return res.getDecompiledFunction().getC();
        } catch (Exception e) { return "decompile fail: " + e; }
        return "(no decompile)";
    }

    void doXref(Address a, boolean decomp) {
        println("=== XREFS to " + a + " ===");
        ReferenceIterator it = rm.getReferencesTo(a);
        LinkedHashSet<Function> funcs = new LinkedHashSet<>();
        int cnt = 0;
        while (it.hasNext() && cnt < 80) {
            Reference r = it.next();
            Address from = r.getFromAddress();
            Function cf = fm.getFunctionContaining(from);
            println("  from " + from + " (" + (cf!=null?cf.getName()+"@"+cf.getEntryPoint():"?") + ")  type=" + r.getReferenceType());
            if (cf != null) funcs.add(cf);
            cnt++;
        }
        if (cnt == 0) println("  (none)");
        if (decomp) for (Function f : funcs) {
            println("----- DECOMP " + f.getName() + " @ " + f.getEntryPoint() + " -----");
            println(decompile(f));
        }
    }

    @Override
    public void run() throws Exception {
        fm = currentProgram.getFunctionManager();
        rm = currentProgram.getReferenceManager();
        mem = currentProgram.getMemory();
        base = currentProgram.getImageBase().getOffset();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);

        for (String spec : getScriptArgs()) {
            String[] p = spec.split(":");
            String kind = p[0];
            Address a = currentProgram.getAddressFactory().getAddress(p[1]);
            if (kind.equals("d")) {
                Function f = fm.getFunctionContaining(a);
                println("=== DECOMP func containing " + a + " : " + (f!=null?f.getName()+"@"+f.getEntryPoint():"?") + " ===");
                if (f != null) println(decompile(f));
            } else if (kind.equals("x")) {
                doXref(a, false);
            } else if (kind.equals("xd")) {
                doXref(a, true);
            } else if (kind.equals("s")) {
                int n = Integer.parseInt(p[2]);
                for (int i = 0; i < n; i++) {
                    Address cur = a.add(i);
                    ReferenceIterator it = rm.getReferencesTo(cur);
                    if (it.hasNext()) {
                        println("--- byte +0x" + Integer.toHexString(i) + " @ " + cur + " has refs:");
                        int c = 0;
                        while (it.hasNext() && c < 40) {
                            Reference r = it.next();
                            Function cf = fm.getFunctionContaining(r.getFromAddress());
                            println("    from " + r.getFromAddress() + " (" + (cf!=null?cf.getName():"?") + ") " + r.getReferenceType());
                            c++;
                        }
                    }
                }
            } else if (kind.equals("rtti")) {
                try {
                    long colp = mem.getLong(a.subtract(8));
                    Address col = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(colp));
                    int tdRva = mem.getInt(col.add(0xc));
                    Address td = currentProgram.getAddressFactory().getAddress("0x"+Long.toHexString(base+(tdRva&0xffffffffL)));
                    StringBuilder sb = new StringBuilder();
                    for (int j=0;j<200;j++){ byte b=mem.getByte(td.add(0x10+j)); if(b==0)break; sb.append((char)(b&0xff)); }
                    println("RTTI @ vt " + a + " : " + sb);
                } catch (Exception e) { println("rtti fail: " + e); }
            } else if (kind.equals("m")) {
                long v = mem.getLong(a);
                println("qword @ " + a + " = 0x" + Long.toHexString(v));
            }
        }
        dec.dispose();
    }
}
