// TipTextMap.java -- pass 1 of loading-screen tip-text pipeline RE (bd er-effects-rs-jsm).
// Decompiles CS::LoadingScreen::Update (dump 0x14090a7a0) and CSNowLoadingHelperImp::Update
// (dump 0x1402a2c40), scans function names for loading/tip-related classes, and byte-searches
// memory for the NowLoading movie/tip identifiers so we can find the SetText/Invoke path.
// Usage: bash scripts/ghidra-query.sh scripts/ghidra/TipTextMap.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;

public class TipTextMap extends GhidraScript {
    DecompInterface di;

    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);

        decomp(0x14090a7a0L, "LoadingScreen::Update(dump)");
        decomp(0x1402a2c40L, "CSNowLoadingHelperImp::Update(dump)");

        println("\n==== FUNCTION NAME SCAN ====");
        String[] subs = { "NowLoading", "LoadingScreen", "LoadingText", "LoadingTitle",
                          "MenuText", "SetText", "TextComponent" };
        FunctionManager fm = currentProgram.getFunctionManager();
        int count = 0;
        for (Function f : fm.getFunctions(true)) {
            String n = f.getName(true);
            for (String s : subs) {
                if (n.contains(s)) {
                    println("  FUNC " + f.getEntryPoint() + "  " + n);
                    count++;
                    break;
                }
            }
            if (count > 400) { println("  (truncated)"); break; }
        }

        println("\n==== SYMBOL SCAN (non-function) ====");
        SymbolTable st = currentProgram.getSymbolTable();
        String[] symSubs = { "NowLoading", "LoadingScreen" };
        int sc = 0;
        for (Symbol s : st.getAllSymbols(false)) {
            String n = s.getName();
            for (String sub : symSubs) {
                if (n.contains(sub)) {
                    println("  SYM " + s.getAddress() + "  " + s.getSymbolType() + "  " + s.getName(true));
                    sc++;
                    break;
                }
            }
            if (sc > 200) { println("  (truncated)"); break; }
        }

        println("\n==== MEMORY STRING HITS ====");
        findStr("02_903", true);
        findStr("02_903", false);
        findStr("NowLoading", true);
        findStr("NowLoading", false);
        findStr("TextTips", true);
        findStr("TextTips", false);
    }

    void decomp(long va, String label) {
        try {
            Address a = toAddr(va);
            Function f = getFunctionContaining(a);
            if (f == null) { println("=== " + label + " 0x" + Long.toHexString(va) + " NO_FUNC ==="); return; }
            println("=== " + label + " -> " + f.getName(true) + " entry=" + f.getEntryPoint() + " ===");
            DecompileResults r = di.decompileFunction(f, 90, monitor);
            if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("(decompile failed)");
        } catch (Exception e) { println("(decomp error " + e.getMessage() + ")"); }
    }

    void findStr(String s, boolean utf16) {
        Memory mem = currentProgram.getMemory();
        byte[] pat;
        if (utf16) {
            pat = new byte[s.length() * 2];
            for (int i = 0; i < s.length(); i++) { pat[i * 2] = (byte) s.charAt(i); pat[i * 2 + 1] = 0; }
        } else pat = s.getBytes();
        byte[] mask = new byte[pat.length];
        java.util.Arrays.fill(mask, (byte) 0xff);
        Address a = currentProgram.getMinAddress();
        int hits = 0;
        while (hits < 25) {
            Address found = mem.findBytes(a, pat, mask, true, monitor);
            if (found == null) break;
            StringBuilder sb = new StringBuilder();
            sb.append("  STR").append(utf16 ? "16" : "8").append(" '").append(s).append("' @ ").append(found);
            Reference[] refs = getReferencesTo(found);
            for (Reference r : refs) {
                Function f = getFunctionContaining(r.getFromAddress());
                sb.append("  <-").append(r.getFromAddress());
                if (f != null) sb.append("(").append(f.getName()).append(")");
            }
            println(sb.toString());
            hits++;
            a = found.add(1);
        }
        if (hits == 0) println("  STR" + (utf16 ? "16" : "8") + " '" + s + "'  (none)");
    }
}
