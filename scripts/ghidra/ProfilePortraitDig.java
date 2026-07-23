// ProfilePortraitDig.java
// ONE batched dig for the now-loading-portrait feature against the persistent 'ermaporch' dump.
// Answers: writer of renderer+0x770/+0x778, refresh decomp, offscreen-drive + submit + compositor
// decomp, the g_GxDrawContext global (setter/clearer), and SYSTEX_Menu_Profile render-into flow.
//
// Run: bash scripts/ghidra-query.sh scripts/ghidra/ProfilePortraitDig.java
//
// NOTE: addresses below are DUMP VAs (Ghidra FUN_ names). Translate to deobf via
// scripts/dump-deobf-shift.py before any CALL/PATCH use.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.*;
import java.util.*;

public class ProfilePortraitDig extends GhidraScript {
    DecompInterface di;
    FunctionManager fm;
    SymbolTable st;
    Memory mem;

    String decomp(Function f, int limitChars) {
        if (f == null) return "(null func)";
        try {
            DecompileResults r = di.decompileFunction(f, 60, monitor);
            if (r != null && r.decompileCompleted()) {
                String c = r.getDecompiledFunction().getC();
                if (limitChars > 0 && c.length() > limitChars) c = c.substring(0, limitChars) + "\n...[trimmed]...\n";
                return c;
            }
        } catch (Exception e) { return "(decompile exception: " + e + ")"; }
        return "(decompile failed)";
    }

    void decompVA(String label, long dumpVA) {
        Address a = toAddr(dumpVA);
        Function f = fm.getFunctionContaining(a);
        println("\n==================== " + label + "  dumpVA=0x" + Long.toHexString(dumpVA)
            + "  ->  " + (f != null ? f.getName() + " entry=" + f.getEntryPoint() : "NO_FUNC") + " ====================");
        println(decomp(f, 9000));
    }

    // scan a code range for store instructions whose op0 (dest) carries the given displacement
    Set<Function> scanWrites(long start, long end, long disp) {
        Set<Function> hit = new LinkedHashSet<>();
        Listing lst = currentProgram.getListing();
        String hex = Long.toHexString(disp);
        String n1 = "+ 0x" + hex + "]";
        String n2 = "+ 0x" + hex + ",";
        InstructionIterator ii = lst.getInstructions(toAddr(start), true);
        int n = 0;
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            if (insn.getAddress().getOffset() >= end) break;
            String txt = insn.toString();
            if (!(txt.contains(n1) || txt.contains(n2))) continue;
            boolean op0 = false;
            try {
                String o0 = insn.getDefaultOperandRepresentation(0);
                if (o0.contains("0x" + hex)) op0 = true;
            } catch (Exception e) {}
            if (!op0) continue; // dest only = a store
            Function f = fm.getFunctionContaining(insn.getAddress());
            println("  STORE +0x" + hex + " @ " + insn.getAddress()
                + " (" + (f != null ? f.getName() : "?") + ") : " + txt);
            if (f != null) hit.add(f);
            if (++n > 100) { println("  ...truncated"); break; }
        }
        return hit;
    }

    // print data-section globals referenced inside a function, plus who WRITES them
    void dataRefsAndWriters(String label, long dumpVA) {
        Address a = toAddr(dumpVA);
        Function f = fm.getFunctionContaining(a);
        if (f == null) { println("\n[dataRefs] " + label + " NO_FUNC @ 0x" + Long.toHexString(dumpVA)); return; }
        println("\n[dataRefs] " + label + " = " + f.getName() + " globals referenced:");
        Set<Address> globals = new LinkedHashSet<>();
        Listing lst = currentProgram.getListing();
        InstructionIterator ii = lst.getInstructions(f.getBody(), true);
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            for (Reference r : insn.getReferencesFrom()) {
                Address to = r.getToAddress();
                if (to == null || !to.isMemoryAddress()) continue;
                long off = to.getOffset();
                // data sections of ER live above ~0x143000000
                if (off >= 0x143000000L && off < 0x150000000L) globals.add(to);
            }
        }
        for (Address g : globals) {
            Symbol gs = getSymbolAt(g);
            println("   global @ " + g + (gs != null ? ("  " + gs.getName()) : ""));
            // who writes this global?
            Reference[] refs = getReferencesTo(g);
            int wc = 0;
            for (Reference r : refs) {
                if (r.getReferenceType().isWrite()) {
                    Function wf = fm.getFunctionContaining(r.getFromAddress());
                    println("      WRITE from " + r.getFromAddress() + (wf != null ? ("  in " + wf.getName(true)) : ""));
                    if (++wc > 12) { println("      ...more writers"); break; }
                }
            }
            if (wc == 0) {
                // fall back: list a few read/write xrefs so the setter can be found
                int rc = 0;
                for (Reference r : refs) {
                    Function wf = fm.getFunctionContaining(r.getFromAddress());
                    println("      xref(" + r.getReferenceType() + ") from " + r.getFromAddress()
                        + (wf != null ? ("  in " + wf.getName(true)) : ""));
                    if (++rc > 12) { println("      ...more xrefs"); break; }
                }
            }
        }
    }

    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        fm = currentProgram.getFunctionManager();
        st = currentProgram.getSymbolTable();
        mem = currentProgram.getMemory();

        println("########## SECTION A: decompile the known players (DUMP VAs) ##########");
        decompVA("REFRESH (deobf 0x9aa680)", 0x1409aa7d0L);
        decompVA("OFFSCREEN-DRIVE (deobf 0xbb8ca0)", 0x140bb8d90L);
        decompVA("OFFSCREEN-SUBMIT FUN_140bb73a0", 0x140bb73a0L);
        decompVA("COMPOSITOR FUN_1409e9ac0", 0x1409e9ac0L);
        decompVA("CTOR CSMenuAsmModelRend 0x140bb8110", 0x140bb8110L);
        decompVA("PERFRAME FUN_140bba820 (gated +0x778)", 0x140bba820L);
        decompVA("PERFRAME FUN_140bba7d0 (gated +0x948 && +0x778)", 0x140bba7d0L);
        decompVA("TEARDOWN-ALL (deobf 0x9b2db0)", 0x1409b2f00L);

        println("\n########## SECTION B: WRITERS of renderer+0x770 (CSModelIns*) and +0x778 (CSChrAsmModelIns*) ##########");
        // Scan the CSMenuAsmModelRend cluster and the refresh cluster for stores to those offsets.
        long[][] ranges = {
            {0x140bb7000L, 0x140bbd000L},  // CSMenuAsmModelRend / ProfModelRend cluster
            {0x1409a9000L, 0x1409ad000L},  // refresh cluster
        };
        Set<Function> writers = new LinkedHashSet<>();
        for (long[] rg : ranges) {
            println("\n-- range 0x" + Long.toHexString(rg[0]) + " .. 0x" + Long.toHexString(rg[1]) + " --");
            println(" [disp 0x770]");
            writers.addAll(scanWrites(rg[0], rg[1], 0x770));
            println(" [disp 0x778]");
            writers.addAll(scanWrites(rg[0], rg[1], 0x778));
        }
        println("\n-- decompiling the unique writer functions (cap 6) --");
        int wc = 0;
        for (Function f : writers) {
            if (wc++ >= 6) { println("[writer cap hit]"); break; }
            println("\n==================== WRITER " + f.getName() + " entry=" + f.getEntryPoint() + " ====================");
            println(decomp(f, 8000));
        }

        println("\n########## SECTION C: g_GxDrawContext global -- referenced by offscreen submit, setter/clearer ##########");
        dataRefsAndWriters("OFFSCREEN-SUBMIT FUN_140bb73a0", 0x140bb73a0L);
        dataRefsAndWriters("OFFSCREEN-DRIVE FUN_140bb8d90", 0x140bb8d90L);
        dataRefsAndWriters("COMPOSITOR FUN_1409e9ac0", 0x1409e9ac0L);

        println("\n########## SECTION D: SYSTEX_Menu_Profile render-into flow ##########");
        String[] terms = { "SYSTEX_Menu_Profile", "SYSTEX_Menu", "ProfileFace", "DummyProfileFace" };
        for (String t : terms) {
            println("\n-- term: " + t + " --");
            try {
                Address found = find(t);
                if (found == null) { println("  (ASCII not found)"); continue; }
                int hits = 0;
                while (found != null && hits < 8) {
                    println("  @ " + found + " : " + readStr(found));
                    Reference[] refs = getReferencesTo(found);
                    int rc = 0;
                    for (Reference r : refs) {
                        Function ff = fm.getFunctionContaining(r.getFromAddress());
                        println("     xref from " + r.getFromAddress() + (ff != null ? ("  in " + ff.getName(true)) : ""));
                        if (++rc > 8) { println("     ...more"); break; }
                    }
                    hits++;
                    found = find(found.add(1), t);
                }
            } catch (Exception e) { println("  search err: " + e); }
        }

        println("\n########## DONE ##########");
    }

    String readStr(Address a) {
        try {
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < 48; i++) {
                byte b = mem.getByte(a.add(i));
                if (b == 0) break;
                sb.append((char) (b & 0xff));
            }
            return sb.toString();
        } catch (Exception e) { return "?"; }
    }
}
