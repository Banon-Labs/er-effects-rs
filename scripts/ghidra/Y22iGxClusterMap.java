// Y22iGxClusterMap -- decompile the native-Windows y22i GX null-resource crash cluster and trace the
// call graph up to the dispatch chokepoint, so ONE guard can cover every consumer.
//
// The two known live crash sites (deobf/live VAs, base 0x140000000):
//   - FUN_141e90290 : classifier, guarded already (live RVA 0x1e90290) -- our ANCHOR.
//   - 0x141e862fd   : second consumer, faults independently.
// The dump's VAs differ from live/deobf VAs by a piecewise-constant per-region shift. We ANCHOR that
// shift on the classifier: we know its live VA is 0x141e90290, so once we locate the same function in
// the dump, shift_live_minus_dump = 0x141e90290 - dumpEntry. Everything else in the same region maps by
// the same shift. We locate the classifier in the dump by content (reads [rcx+0x10] then +0x30) among
// candidates near the estimated dump VA.
//
// Usage: analyzeHeadless <proj> <name> -process -noanalysis -readOnly -postScript Y22iGxClusterMap.java
// (run via scripts/ghidra-query.sh once that wrapper is pointed at the y22i project, or directly).

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceManager;

import java.util.ArrayList;
import java.util.List;

public class Y22iGxClusterMap extends GhidraScript {

    // Live/deobf VAs of interest (base 0x140000000).
    static final long CLASSIFIER_LIVE = 0x141e90290L;   // anchor (known live)
    static final long CONSUMER2_LIVE  = 0x141e862fdL;   // second consumer fault site
    // Estimated region shift (CLAUDE.md: ~ -0x20 across 0x140e-0x141e => dump = live + 0x20). Used only
    // as a search hint; the true shift is re-derived from the classifier anchor below.
    static final long SHIFT_HINT = 0x20;

    DecompInterface decomp;

    @Override
    public void run() throws Exception {
        decomp = new DecompInterface();
        decomp.openProgram(currentProgram);

        long imageBase = currentProgram.getImageBase().getOffset();
        println("== program: " + currentProgram.getName() + " imageBase=0x" + Long.toHexString(imageBase));

        // 1) Locate the classifier in the dump near (CLASSIFIER_LIVE + SHIFT_HINT) and derive the exact shift.
        Function classifier = functionCoveringOrNear(CLASSIFIER_LIVE + SHIFT_HINT, 0x200);
        if (classifier == null) {
            println("!! classifier not found near estimate; dumping all functions in [0x141e80000,0x141ea0000)");
            listFunctionsInRange(0x141e80000L, 0x141ea0000L);
            return;
        }
        long shift = CLASSIFIER_LIVE - classifier.getEntryPoint().getOffset(); // live = dump + shift
        println("ANCHOR classifier dumpEntry=0x" + Long.toHexString(classifier.getEntryPoint().getOffset())
                + " name=" + classifier.getName()
                + " => shift(live-dump)=0x" + Long.toHexString(shift)
                + " (live 0x" + Long.toHexString(CLASSIFIER_LIVE) + ")");
        dumpFunction("CLASSIFIER (consumer #1)", classifier, shift);

        // 2) Locate the second consumer function (contains live 0x141e862fd -> dump = live - shift).
        long consumer2Dump = CONSUMER2_LIVE - shift;
        Function consumer2 = functionCoveringOrNear(consumer2Dump, 0x400);
        if (consumer2 != null) {
            dumpFunction("CONSUMER #2 (live 0x" + Long.toHexString(CONSUMER2_LIVE) + " -> dump 0x"
                    + Long.toHexString(consumer2Dump) + ")", consumer2, shift);
        } else {
            println("!! consumer2 not found near dump 0x" + Long.toHexString(consumer2Dump));
        }

        // 3) Trace callers of BOTH consumers to find the common dispatch chokepoint that loads the
        //    resource (wrapper+0x40) and passes it in rcx.
        println("\n== CALLERS of classifier ==");
        List<Function> c1callers = callersOf(classifier);
        for (Function f : c1callers) {
            liveHdr("caller", f, shift);
        }
        if (consumer2 != null) {
            println("\n== CALLERS of consumer #2 ==");
            for (Function f : callersOf(consumer2)) {
                liveHdr("caller", f, shift);
            }
        }

        // 4) Decompile the classifier's direct callers (the likely resource-loading dispatchers) so we can
        //    see where rcx (the resource) comes from -- i.e. the chokepoint that reads wrapper+0x40.
        println("\n== DECOMPILE classifier callers (find the wrapper+0x40 load site) ==");
        int n = 0;
        for (Function f : c1callers) {
            if (n++ >= 4) { println("(more callers omitted)"); break; }
            dumpFunction("CALLER", f, shift);
        }
        decomp.dispose();
    }

    void liveHdr(String tag, Function f, long shift) {
        long dump = f.getEntryPoint().getOffset();
        println(tag + " dumpEntry=0x" + Long.toHexString(dump)
                + " liveEntry=0x" + Long.toHexString(dump + shift)
                + " liveRVA=0x" + Long.toHexString(dump + shift - 0x140000000L)
                + " name=" + f.getName());
    }

    Function functionCoveringOrNear(long dumpVa, long window) {
        Address a = toAddr(dumpVa);
        Function f = getFunctionContaining(a);
        if (f != null) return f;
        FunctionManager fm = currentProgram.getFunctionManager();
        Function best = null; long bestD = Long.MAX_VALUE;
        Function it = fm.getFunctionContaining(a);
        if (it != null) return it;
        // scan nearby function entries
        for (Function g : fm.getFunctions(true)) {
            long e = g.getEntryPoint().getOffset();
            long d = Math.abs(e - dumpVa);
            if (d < bestD && d <= window) { bestD = d; best = g; }
        }
        return best;
    }

    List<Function> callersOf(Function f) {
        List<Function> out = new ArrayList<>();
        ReferenceManager rm = currentProgram.getReferenceManager();
        for (Reference r : rm.getReferencesTo(f.getEntryPoint())) {
            Function c = getFunctionContaining(r.getFromAddress());
            if (c != null && !out.contains(c)) out.add(c);
        }
        return out;
    }

    void dumpFunction(String tag, Function f, long shift) {
        long dump = f.getEntryPoint().getOffset();
        println("\n---- " + tag + " ---- dumpEntry=0x" + Long.toHexString(dump)
                + " liveEntry=0x" + Long.toHexString(dump + shift)
                + " liveRVA=0x" + Long.toHexString(dump + shift - 0x140000000L)
                + " name=" + f.getName()
                + " sig=" + f.getSignature().getPrototypeString());
        try {
            DecompileResults dr = decomp.decompileFunction(f, 45, monitor);
            if (dr != null && dr.decompileCompleted()) {
                String c = dr.getDecompiledFunction().getC();
                // Cap to keep output readable.
                if (c.length() > 6000) c = c.substring(0, 6000) + "\n/* ...truncated... */";
                println(c);
            } else {
                println("(decompile failed)");
            }
        } catch (Exception e) {
            println("(decompile exception: " + e.getMessage() + ")");
        }
    }

    void listFunctionsInRange(long lo, long hi) {
        FunctionManager fm = currentProgram.getFunctionManager();
        for (Function g : fm.getFunctions(true)) {
            long e = g.getEntryPoint().getOffset();
            if (e >= lo && e < hi) {
                println("fn 0x" + Long.toHexString(e) + " " + g.getName());
            }
        }
    }
}
