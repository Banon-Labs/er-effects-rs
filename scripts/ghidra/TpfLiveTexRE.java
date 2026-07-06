// TpfLiveTexRE.java
// One batched dig for the live-portrait-inside-GFx question (bd er-effects-rs-jsm).
// Answers, against the persistent 'ermaporch' dump (DUMP VAs -- translate to deobf via
// scripts/dump-deobf-shift.py before any CALL/PATCH use):
//   A. CreateTpfResCap chain down to the GPU texture build (heap/state semantics).
//   B. TexRepositoryImp::InsertCSRuntimeTexResCapIfNotExists (dump 0x1401e9760) + its callers
//      (how native menus register engine RTs so GFx can sample them).
//   C. Scaleform->CS repo bridge FUN_140d66220 + TexRepositoryImp::GetResCap (dump 0x140b80a90)
//      (what descriptor/state work happens when GFx wraps a CSGxTexture).
//   D. CSGxTexture / CSRuntimeTexResCap symbols + RTTI to pin the layout.
//
// Run: bash scripts/ghidra-query.sh scripts/ghidra/TpfLiveTexRE.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.Memory;
import java.util.*;

public class TpfLiveTexRE extends GhidraScript {
    DecompInterface di;
    FunctionManager fm;
    SymbolTable st;
    Memory mem;

    String decomp(Function f, int limitChars) {
        if (f == null) return "(null func)";
        try {
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r != null && r.decompileCompleted()) {
                String c = r.getDecompiledFunction().getC();
                if (limitChars > 0 && c.length() > limitChars) c = c.substring(0, limitChars) + "\n...[trimmed]...\n";
                return c;
            }
        } catch (Exception e) { return "(decompile exception: " + e + ")"; }
        return "(decompile failed)";
    }

    Function fAt(long va) {
        Address a = toAddr(va);
        Function f = fm.getFunctionAt(a);
        if (f == null) f = fm.getFunctionContaining(a);
        return f;
    }

    void decompVA(String label, long dumpVA, int cap) {
        Function f = fAt(dumpVA);
        println("\n==================== " + label + "  dumpVA=0x" + Long.toHexString(dumpVA)
            + "  ->  " + (f != null ? f.getName(true) + " entry=" + f.getEntryPoint() : "NO_FUNC") + " ====================");
        println(decomp(f, cap));
    }

    void listCallees(String label, long dumpVA) {
        Function f = fAt(dumpVA);
        if (f == null) { println("[callees] " + label + ": NO_FUNC"); return; }
        println("\n[callees of " + label + " = " + f.getName(true) + "]");
        Set<Function> callees = new LinkedHashSet<>();
        InstructionIterator ii = currentProgram.getListing().getInstructions(f.getBody(), true);
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            for (Reference r : insn.getReferencesFrom()) {
                if (!r.getReferenceType().isCall()) continue;
                Function cf = fm.getFunctionAt(r.getToAddress());
                if (cf != null) callees.add(cf);
            }
        }
        for (Function cf : callees) println("   -> " + cf.getEntryPoint() + "  " + cf.getName(true));
    }

    void callersOf(String label, long dumpVA, int decompCap, int maxDecomp) {
        Function f = fAt(dumpVA);
        if (f == null) { println("[callers] " + label + ": NO_FUNC"); return; }
        println("\n[callers of " + label + " = " + f.getName(true) + " @ " + f.getEntryPoint() + "]");
        Reference[] refs = getReferencesTo(f.getEntryPoint());
        List<Function> callers = new ArrayList<>();
        Set<Address> seen = new HashSet<>();
        for (Reference r : refs) {
            if (!r.getReferenceType().isCall()) continue;
            Function cf = fm.getFunctionContaining(r.getFromAddress());
            println("   call from " + r.getFromAddress() + (cf != null ? ("  in " + cf.getName(true) + " entry=" + cf.getEntryPoint()) : ""));
            if (cf != null && seen.add(cf.getEntryPoint())) callers.add(cf);
        }
        int n = 0;
        for (Function cf : callers) {
            if (n++ >= maxDecomp) { println("[caller decomp cap hit]"); break; }
            println("\n-------------------- CALLER " + cf.getName(true) + " entry=" + cf.getEntryPoint() + " --------------------");
            println(decomp(cf, decompCap));
        }
    }

    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        fm = currentProgram.getFunctionManager();
        st = currentProgram.getSymbolTable();
        mem = currentProgram.getMemory();

        println("########## SECTION A: TPF -> GPU texture creation chain ##########");
        decompVA("CreateTpfResCap", 0x140b83770L, 9000);
        decompVA("TpfResCap fill FUN_140b83ec0", 0x140b83ec0L, 9000);
        decompVA("GXCGTextureBuilder_TPF (deobf 0x141a004c0)", 0x141a004e0L, 9000);
        decompVA("builder-name FUN_141a00950", 0x141a00950L, 5000);
        decompVA("register-into-TexRepository FUN_140b81110", 0x140b81110L, 9000);
        listCallees("GXCGTextureBuilder_TPF", 0x141a004e0L);
        listCallees("FUN_140b81110", 0x140b81110L);

        println("\n########## SECTION B: InsertCSRuntimeTexResCapIfNotExists (dump 0x1401e9760) ##########");
        decompVA("InsertCSRuntimeTexResCapIfNotExists", 0x1401e9760L, 9000);
        listCallees("InsertCSRuntimeTexResCapIfNotExists", 0x1401e9760L);
        callersOf("InsertCSRuntimeTexResCapIfNotExists", 0x1401e9760L, 8000, 6);

        println("\n########## SECTION C: Scaleform bridge + GetResCap ##########");
        decompVA("Scaleform miss-bridge FUN_140d66220", 0x140d66220L, 9000);
        decompVA("TexRepositoryImp::GetResCap", 0x140b80a90L, 7000);

        println("\n########## SECTION D: CSGxTexture / CSRuntimeTexResCap / OffscreenRend symbols ##########");
        String[] pats = { "CSGxTex", "GxTexture", "RuntimeTexResCap", "EzOffscreen", "OffscreenGxTexture" };
        SymbolIterator si = st.getSymbolIterator();
        int hits = 0;
        while (si.hasNext() && hits < 400) {
            Symbol s = si.next();
            String nm = s.getName(true);
            for (String p : pats) {
                if (nm.contains(p)) {
                    println("SYM " + s.getAddress() + "  [" + s.getSymbolType() + "] " + nm);
                    hits++;
                    break;
                }
            }
        }

        println("\n########## DONE ##########");
    }
}
