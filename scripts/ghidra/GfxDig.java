// GfxDig.java -- generic batched RE dig against the persistent 'ermaporch' dump.
// All addresses are DUMP VAs (translate to deobf via scripts/dump-deobf-shift.py before
// any CALL/PATCH use). Run:
//   bash scripts/ghidra-query.sh scripts/ghidra/GfxDig.java <cmd> [<cmd> ...]
// Commands (processed in order):
//   d:0xVA[:CAP]   decompile the function containing VA (CAP chars, default 20000)
//   c:0xVA         list call-callees of the function containing VA
//   x:0xVA[:MAX]   list references TO VA (default max 60), with containing function
//   v:0xVA:N       dump N 8-byte pointer entries at VA (vtable), naming target functions
//   s:SUBSTR       symbol-name substring search (max 120 hits)
//   b:0xVA:N       hex dump N bytes at VA
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.Memory;
import java.util.*;

public class GfxDig extends GhidraScript {
    DecompInterface di;
    FunctionManager fm;
    Memory mem;

    Function fAt(long va) {
        Address a = toAddr(va);
        Function f = fm.getFunctionAt(a);
        if (f == null) f = fm.getFunctionContaining(a);
        return f;
    }

    void decompVA(long va, int cap) {
        Function f = fAt(va);
        println("\n==== DECOMP 0x" + Long.toHexString(va) + " -> "
            + (f != null ? f.getName(true) + " entry=" + f.getEntryPoint() : "NO_FUNC") + " ====");
        if (f == null) return;
        try {
            DecompileResults r = di.decompileFunction(f, 120, monitor);
            if (r != null && r.decompileCompleted()) {
                String c = r.getDecompiledFunction().getC();
                if (cap > 0 && c.length() > cap) c = c.substring(0, cap) + "\n...[trimmed]...";
                println(c);
            } else println("(decompile failed)");
        } catch (Exception e) { println("(decompile exception: " + e + ")"); }
    }

    void callees(long va) {
        Function f = fAt(va);
        println("\n==== CALLEES 0x" + Long.toHexString(va) + " -> "
            + (f != null ? f.getName(true) : "NO_FUNC") + " ====");
        if (f == null) return;
        Set<String> seen = new LinkedHashSet<>();
        InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            for (Reference r : insn.getReferencesFrom()) {
                if (!r.getReferenceType().isCall()) continue;
                Function cf = fm.getFunctionAt(r.getToAddress());
                String line = "  " + insn.getAddress() + " -> " + r.getToAddress() + "  "
                    + (cf != null ? cf.getName(true) : "(no func)");
                if (seen.add(line)) println(line);
            }
        }
    }

    void xrefs(long va, int max) {
        Address a = toAddr(va);
        println("\n==== XREFS TO 0x" + Long.toHexString(va) + " ====");
        int n = 0;
        for (Reference r : getReferencesTo(a)) {
            if (n++ >= max) { println("  [cap hit]"); break; }
            Function cf = fm.getFunctionContaining(r.getFromAddress());
            println("  " + r.getFromAddress() + " [" + r.getReferenceType() + "]"
                + (cf != null ? "  in " + cf.getName(true) + " entry=" + cf.getEntryPoint() : ""));
        }
    }

    void vtable(long va, int n) {
        println("\n==== VTABLE DUMP 0x" + Long.toHexString(va) + " (" + n + " slots) ====");
        try {
            for (int i = 0; i < n; i++) {
                Address slot = toAddr(va + 8L * i);
                long p = mem.getLong(slot);
                Function f = fm.getFunctionAt(toAddr(p));
                Symbol s = currentProgram.getSymbolTable().getPrimarySymbol(toAddr(p));
                println("  +0x" + Long.toHexString(8L * i) + "  0x" + Long.toHexString(p) + "  "
                    + (f != null ? f.getName(true) : (s != null ? s.getName(true) : "")));
            }
        } catch (Exception e) { println("  (read exception: " + e + ")"); }
    }

    void symSearch(String sub) {
        println("\n==== SYMBOL SEARCH '" + sub + "' ====");
        SymbolIterator si = currentProgram.getSymbolTable().getSymbolIterator();
        int hits = 0;
        while (si.hasNext() && hits < 120) {
            Symbol s = si.next();
            String nm = s.getName(true);
            if (nm.contains(sub)) { println("  " + s.getAddress() + " [" + s.getSymbolType() + "] " + nm); hits++; }
        }
    }

    void hexdump(long va, int n) {
        println("\n==== BYTES 0x" + Long.toHexString(va) + " (" + n + ") ====");
        try {
            byte[] b = new byte[n];
            mem.getBytes(toAddr(va), b);
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < n; i++) {
                if (i % 16 == 0) sb.append(String.format("%n  %08x: ", va + i));
                sb.append(String.format("%02x ", b[i] & 0xff));
            }
            println(sb.toString());
        } catch (Exception e) { println("  (read exception: " + e + ")"); }
    }

    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        fm = currentProgram.getFunctionManager();
        mem = currentProgram.getMemory();
        for (String arg : getScriptArgs()) {
            String[] p = arg.split(":");
            try {
                switch (p[0]) {
                    case "d": decompVA(Long.decode(p[1]), p.length > 2 ? Integer.parseInt(p[2]) : 20000); break;
                    case "c": callees(Long.decode(p[1])); break;
                    case "x": xrefs(Long.decode(p[1]), p.length > 2 ? Integer.parseInt(p[2]) : 60); break;
                    case "v": vtable(Long.decode(p[1]), Integer.parseInt(p[2])); break;
                    case "s": symSearch(p[1]); break;
                    case "b": hexdump(Long.decode(p[1]), Integer.parseInt(p[2])); break;
                    default: println("UNKNOWN CMD: " + arg);
                }
            } catch (Exception e) { println("CMD FAIL " + arg + ": " + e); }
        }
        println("\n==== GFXDIG DONE ====");
    }
}
