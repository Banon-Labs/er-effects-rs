// FocusGateDig.java
// Investigate window-active / focus gating of GAMEPLAY (locomotion) input.
// Actions per arg:
//   d:0xADDR         -> decompile function containing ADDR (dump VA)
//   n:NAME           -> find symbols whose name contains NAME; for each, list xref callers + containing funcs
//   nd:NAME          -> like n: but ALSO decompile each distinct caller function
//   x:0xADDR         -> list xrefs to ADDR (with ref type + containing func)
// If no args, runs a built-in default battery.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.util.task.ConsoleTaskMonitor;
import java.util.*;

public class FocusGateDig extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;
    ReferenceManager rm;
    SymbolTable st;
    Memory mem;
    long base;

    String decompile(Function f) {
        try {
            DecompileResults res = dec.decompileFunction(f, 120, new ConsoleTaskMonitor());
            if (res != null && res.decompileCompleted())
                return res.getDecompiledFunction().getC();
        } catch (Exception e) { return "decompile fail: " + e; }
        return "(no decompile)";
    }

    Address va(String s) { return currentProgram.getAddressFactory().getAddress(s); }

    void decompAt(String addr) {
        Address a = va(addr);
        Function f = fm.getFunctionContaining(a);
        println("\n========== DECOMP func containing " + a + " : "
                + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "??? (no func)") + " ==========");
        if (f != null) println(decompile(f));
    }

    void doName(String name, boolean decomp) {
        println("\n########## SYMBOL SEARCH: '" + name + "' ##########");
        SymbolIterator sit = st.getSymbolIterator();
        List<Symbol> matches = new ArrayList<>();
        while (sit.hasNext()) {
            Symbol s = sit.next();
            if (s.getName().toLowerCase().contains(name.toLowerCase())) matches.add(s);
        }
        if (matches.isEmpty()) { println("  (no symbol matches)"); return; }
        LinkedHashSet<Function> callers = new LinkedHashSet<>();
        for (Symbol s : matches) {
            println("-- symbol " + s.getName() + " @ " + s.getAddress() + " type=" + s.getSymbolType());
            ReferenceIterator it = rm.getReferencesTo(s.getAddress());
            int cnt = 0;
            while (it.hasNext() && cnt < 60) {
                Reference r = it.next();
                Address from = r.getFromAddress();
                Function cf = fm.getFunctionContaining(from);
                println("    xref from " + from + " (" + (cf != null ? cf.getName() + "@" + cf.getEntryPoint() : "?")
                        + ") type=" + r.getReferenceType());
                if (cf != null) callers.add(cf);
                cnt++;
            }
            if (cnt == 0) println("    (no xrefs)");
        }
        if (decomp) for (Function f : callers) {
            println("\n----- DECOMP CALLER " + f.getName() + " @ " + f.getEntryPoint() + " -----");
            println(decompile(f));
        }
    }

    void doCalls(String addr) {
        Address a = va(addr);
        Function f = fm.getFunctionContaining(a);
        println("\n########## CALLS in " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "?") + " ##########");
        if (f == null) return;
        ghidra.program.model.listing.Listing lst = currentProgram.getListing();
        InstructionIterator ii = lst.getInstructions(f.getBody(), true);
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            for (Reference r : insn.getReferencesFrom()) {
                if (r.getReferenceType().isCall()) {
                    Function tf = fm.getFunctionAt(r.getToAddress());
                    println("  CALL " + insn.getAddress() + " -> " + r.getToAddress()
                            + (tf != null ? "  " + tf.getName() : ""));
                }
            }
        }
    }

    void doExactSym(String name, boolean decomp) {
        println("\n########## EXACT SYMBOL: '" + name + "' ##########");
        java.util.List<Symbol> gs = st.getGlobalSymbols(name);
        for (Symbol s : gs) {
            println("  global " + s.getName() + " @ " + s.getAddress() + " type=" + s.getSymbolType());
            if (decomp) {
                Function f = fm.getFunctionContaining(s.getAddress());
                if (f != null) { println("  ---- DECOMP ----"); println(decompile(f)); }
            }
        }
        SymbolIterator it = st.getSymbols(name);
        while (it.hasNext()) {
            Symbol s = it.next();
            println("  any " + s.getName() + " @ " + s.getAddress() + " type=" + s.getSymbolType()
                    + " ns=" + s.getParentNamespace().getName());
            if (decomp) {
                Function f = fm.getFunctionContaining(s.getAddress());
                if (f != null) { println("  ---- DECOMP ----"); println(decompile(f)); }
            }
        }
    }

    void doQuery(String glob, boolean decomp) {
        println("\n########## GLOB SYMBOL QUERY: '" + glob + "' ##########");
        SymbolIterator it = st.getSymbolIterator(glob, true);
        int n = 0;
        while (it.hasNext() && n < 40) {
            Symbol s = it.next();
            println("  " + s.getName(true) + " @ " + s.getAddress() + " type=" + s.getSymbolType());
            n++;
            if (decomp) {
                Function f = fm.getFunctionContaining(s.getAddress());
                if (f != null) { println("  ---- DECOMP ----"); println(decompile(f)); }
            }
        }
        println("  matches=" + n);
    }

    void doXref(String addr) {
        Address a = va(addr);
        println("\n########## XREFS to " + a + " ##########");
        ReferenceIterator it = rm.getReferencesTo(a);
        int cnt = 0;
        while (it.hasNext() && cnt < 100) {
            Reference r = it.next();
            Function cf = fm.getFunctionContaining(r.getFromAddress());
            println("  from " + r.getFromAddress() + " (" + (cf != null ? cf.getName() + "@" + cf.getEntryPoint() : "?")
                    + ") type=" + r.getReferenceType());
            cnt++;
        }
        if (cnt == 0) println("  (none)");
    }

    @Override
    public void run() throws Exception {
        fm = currentProgram.getFunctionManager();
        rm = currentProgram.getReferenceManager();
        st = currentProgram.getSymbolTable();
        mem = currentProgram.getMemory();
        base = currentProgram.getImageBase().getOffset();
        dec = new DecompInterface();
        DecompileOptions opts = new DecompileOptions();
        dec.setOptions(opts);
        dec.openProgram(currentProgram);

        String[] args = getScriptArgs();
        if (args.length == 0) {
            // Default battery.
            decompAt("0x141f292bd");   // menu-byte clear region
            decompAt("0x141f6bad0");   // pad-device poll
            decompAt("0x142667c60");   // FD4PadManager::Update
            decompAt("0x141f295b0");   // DLUserInputManager A
            decompAt("0x141f29010");   // DLUserInputManager B
            doName("GetActiveWindow", true);
            doName("GetForegroundWindow", true);
        } else {
            for (String spec : args) {
                int c = spec.indexOf(':');
                String kind = spec.substring(0, c);
                String rest = spec.substring(c + 1);
                if (kind.equals("d")) decompAt(rest);
                else if (kind.equals("n")) doName(rest, false);
                else if (kind.equals("nd")) doName(rest, true);
                else if (kind.equals("x")) doXref(rest);
                else if (kind.equals("c")) doCalls(rest);
                else if (kind.equals("sy")) doExactSym(rest, false);
                else if (kind.equals("syd")) doExactSym(rest, true);
                else if (kind.equals("q")) doQuery(rest, false);
                else if (kind.equals("qd")) doQuery(rest, true);
                else println("unknown kind: " + kind);
            }
        }
        dec.dispose();
        println("\n=== FocusGateDig DONE ===");
    }
}
