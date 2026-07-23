import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import java.util.*;

// Hunt for the boot-time Privacy Policy / ToS / EULA full-screen agreement dialog.
// Phase 1: enumerate symbols/classes whose name hints at the legal/agreement dialog.
// Phase 2: scan instruction operands for the FMG text IDs (607200 privacy header,
//          607202 privacy body, 607001 Accept, 607000 footer, 606300/607300 consent)
//          and report the containing functions (these reference the dialog text).
public class PrivacyPolicyHunt extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;

    String fname(Function f) {
        if (f == null) return "<null>";
        String nm = f.getName();
        try { nm = (f.getParentNamespace()!=null? f.getParentNamespace().getName(true)+"::":"") + nm; } catch (Exception e) {}
        return nm;
    }

    public void run() throws Exception {
        fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();
        Listing listing = currentProgram.getListing();
        println("[PPH] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        // ---- Phase 1: name-based symbol hunt ----
        String[] needles = {"Tos","TOS","Eula","EULA","Privacy","Agreement","MultiLang",
                            "Legal","Consent","License","Terms","TitleMenu","BootMenu",
                            "AgreeMenu","ToS"};
        println("[PPH] ===== PHASE1 symbol name hunt =====");
        Set<String> seen = new HashSet<>();
        SymbolIterator sit = st.getAllSymbols(true);
        int cap = 0;
        while (sit.hasNext() && cap < 400) {
            Symbol s = sit.next();
            String n = s.getName();
            for (String nd : needles) {
                if (n.contains(nd)) {
                    String key = n + "@" + s.getAddress();
                    if (seen.add(key)) {
                        println(String.format("[PPH-SYM] %-50s @ %s  (ns=%s) type=%s",
                            n, s.getAddress(), s.getParentNamespace()!=null?s.getParentNamespace().getName(true):"-", s.getSymbolType()));
                        cap++;
                    }
                    break;
                }
            }
        }
        println("[PPH] phase1 symbol hits="+cap);

        // ---- Phase 2: scan code for immediate operands equal to the text IDs ----
        long[] ids = {607200L,607201L,607202L,607001L,607002L,607000L,607100L,607102L,
                      606300L,607300L,607004L};
        Set<Long> idset = new HashSet<>();
        for (long id : ids) idset.add(id);
        // map id -> set of function entry offsets
        Map<Long,Set<Long>> hits = new TreeMap<>();
        for (long id : ids) hits.put(id, new TreeSet<>());

        println("[PPH] ===== PHASE2 scanning instructions for text-id immediates =====");
        InstructionIterator iit = listing.getInstructions(true);
        long count = 0;
        while (iit.hasNext()) {
            Instruction insn = iit.next();
            count++;
            int nop = insn.getNumOperands();
            for (int oi = 0; oi < nop; oi++) {
                Object[] objs = insn.getOpObjects(oi);
                for (Object o : objs) {
                    if (o instanceof Scalar) {
                        long v = ((Scalar)o).getUnsignedValue();
                        if (idset.contains(v)) {
                            Function f = fm.getFunctionContaining(insn.getAddress());
                            long ent = f!=null? f.getEntryPoint().getOffset() : 0;
                            hits.get(v).add(ent);
                            println(String.format("[PPH-ID] id=%d at %s  in %s @0x%x  : %s",
                                v, insn.getAddress(), fname(f), ent, insn.toString()));
                        }
                    }
                }
            }
        }
        println("[PPH] scanned "+count+" instructions");
        println("[PPH] ===== PHASE2 summary =====");
        for (long id : ids) {
            StringBuilder sb = new StringBuilder();
            for (long e : hits.get(id)) sb.append(String.format("0x%x ", e));
            println(String.format("[PPH-SUM] id=%d funcs=[%s]", id, sb.toString().trim()));
        }
        println("[PPH] DONE");
    }
}
