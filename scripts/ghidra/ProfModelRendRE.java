// ProfModelRendRE.java
// One-shot batched RE of CSMenuProfModelRend in the persistent 'ermaporch' dump project.
// Resolves: class/namespace, vtable, member functions (decompiled), field semantics for
// +0x754/+0x755/+0x778/+0x9a8/+0xa8, and whether SYSTEX_Menu_Profile/DummyProfileFace exist
// as separate baked textures bound by the ProfileSelect dialog.
//
// Run: bash scripts/ghidra-query.sh scripts/ghidra/ProfModelRendRE.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.Memory;
import java.util.*;

public class ProfModelRendRE extends GhidraScript {
    DecompInterface di;

    String decomp(Function f, int limitChars) {
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

    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        FunctionManager fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();
        Memory mem = currentProgram.getMemory();

        println("########## SECTION 1: symbols/functions containing ProfModelRend / MenuProf ##########");
        List<Function> members = new ArrayList<>();
        Set<Address> seen = new HashSet<>();
        try {
            for (Function f : fm.getFunctions(true)) {
                String n = f.getName();
                String full = f.getName(true); // include namespace
                if (full.contains("ProfModelRend") || full.contains("MenuProfModel")) {
                    println("FUNC " + f.getEntryPoint() + "  " + full);
                    if (seen.add(f.getEntryPoint())) members.add(f);
                }
            }
        } catch (Exception e) { println("func scan err: " + e); }

        println("\n-- non-function symbols (vftable/RTTI/data) matching ProfModelRend --");
        try {
            SymbolIterator si = st.getSymbolIterator();
            while (si.hasNext()) {
                Symbol s = si.next();
                String nm = s.getName(true);
                if (nm.contains("ProfModelRend") || nm.contains("MenuProfModel")) {
                    if (s.getSymbolType() != SymbolType.FUNCTION) {
                        println("SYM " + s.getAddress() + "  [" + s.getSymbolType() + "] " + nm);
                    }
                }
            }
        } catch (Exception e) { println("symbol scan err: " + e); }

        println("\n########## SECTION 2: vtable resolution + slot functions ##########");
        // Find a vftable symbol for the class; dump its slots.
        try {
            List<Address> vtAddrs = new ArrayList<>();
            SymbolIterator si = st.getSymbolIterator();
            while (si.hasNext()) {
                Symbol s = si.next();
                String nm = s.getName(true);
                if ((nm.contains("ProfModelRend")) && (nm.contains("vftable") || nm.contains("vtable") || nm.contains("vbtable"))) {
                    vtAddrs.add(s.getAddress());
                    println("VTABLE SYM " + s.getAddress() + "  " + nm);
                }
            }
            for (Address vt : vtAddrs) {
                println("-- slots @ " + vt + " --");
                for (int i = 0; i < 40; i++) {
                    Address slot = vt.add((long) i * 8);
                    long p;
                    try { p = mem.getLong(slot); } catch (Exception e) { break; }
                    if (p == 0) { println("[" + i + "] 0"); continue; }
                    Address tgt = toAddr(p);
                    Function f = fm.getFunctionAt(tgt);
                    if (f == null) f = fm.getFunctionContaining(tgt);
                    String fn = f != null ? f.getName(true) : "?";
                    println("[" + i + "] -> 0x" + Long.toHexString(p) + "  " + fn);
                    if (f != null && seen.add(f.getEntryPoint())) members.add(f);
                }
            }
        } catch (Exception e) { println("vtable err: " + e); }

        println("\n########## SECTION 3: decompiled member functions (look for +0x754/0x755/0x778/0x9a8/0xa8) ##########");
        int cap = 0;
        for (Function f : members) {
            if (cap++ > 30) { println("[member cap hit, stopping decomp]"); break; }
            println("\n==================== " + f.getName(true) + "  entry=" + f.getEntryPoint() + " ====================");
            println(decomp(f, 9000));
        }

        println("\n########## SECTION 4: baked-texture string search (SYSTEX_Menu_Profile / DummyProfileFace / ProfModelRend) ##########");
        String[] terms = { "SYSTEX_Menu_Profile", "DummyProfileFace", "ProfModelRend", "MenuProfile", "ProfileFace" };
        for (String t : terms) {
            println("\n-- term: " + t + " --");
            try {
                Address found = find(t); // ASCII search
                if (found == null) { println("  (ASCII not found)"); }
                int hits = 0;
                while (found != null && hits < 6) {
                    println("  @ " + found);
                    // xrefs to this string addr
                    Reference[] refs = getReferencesTo(found);
                    int rc = 0;
                    for (Reference r : refs) {
                        Address from = r.getFromAddress();
                        Function ff = fm.getFunctionContaining(from);
                        println("     xref from " + from + (ff != null ? ("  in " + ff.getName(true)) : ""));
                        if (++rc > 8) { println("     ...more xrefs"); break; }
                    }
                    hits++;
                    found = find(found.add(1), t);
                }
            } catch (Exception e) { println("  search err: " + e); }
        }

        println("\n########## DONE ##########");
    }
}
