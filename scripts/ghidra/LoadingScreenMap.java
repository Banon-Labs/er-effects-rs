// Dumps class/namespace members + vtables for the loading-screen render path.
// Usage: bash scripts/ghidra-query.sh scripts/ghidra/LoadingScreenMap.java
import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import java.util.*;

public class LoadingScreenMap extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] classes = {
            "CSFakeLoadingScreenImp", "CSFakeLoadingScreen", "CSDrawStep",
            "CSNowLoadingHelperImp", "CSAdhocDraw", "CSScaleformReplaceTexInfo",
            "CSScaleform", "CSLod", "CSLodImp"
        };
        SymbolTable st = currentProgram.getSymbolTable();
        Listing listing = currentProgram.getListing();
        Memory mem = currentProgram.getMemory();
        for (String cn : classes) {
            println("==== NAMESPACE " + cn + " ====");
            // find namespace symbols
            List<Namespace> nss = new ArrayList<>();
            SymbolIterator it = st.getSymbols(cn);
            while (it.hasNext()) {
                Symbol s = it.next();
                if (s.getSymbolType() == SymbolType.NAMESPACE || s.getSymbolType() == SymbolType.CLASS) {
                    Object o = s.getObject();
                    if (o instanceof Namespace) nss.add((Namespace) o);
                }
            }
            if (nss.isEmpty()) { println("  (no namespace found)"); }
            for (Namespace ns : nss) {
                SymbolIterator mi = st.getSymbols(ns);
                List<Symbol> members = new ArrayList<>();
                while (mi.hasNext()) members.add(mi.next());
                members.sort(Comparator.comparing(s -> s.getName()));
                for (Symbol s : members) {
                    String kind = s.getSymbolType().toString();
                    println(String.format("  %-10s %-50s @ %s", kind, s.getName(), s.getAddress()));
                    // if this is a vftable, dump pointers
                    if (s.getName().toLowerCase().contains("vftable") || s.getName().toLowerCase().contains("vtable")) {
                        dumpVtable(mem, st, listing, s.getAddress());
                    }
                }
            }
        }

        // CSNowLoadingHelperImp::Update specifically (referenced by ctor as updateFn)
        println("\n==== resolve CS::CSNowLoadingHelperImp::Update ====");
        for (Symbol s : st.getSymbols("Update")) {
            Namespace ns = s.getParentNamespace();
            if (ns != null && ns.getName().contains("NowLoadingHelper")) {
                println("  Update @ " + s.getAddress() + "  parent=" + ns.getName(true));
            }
        }
    }

    void dumpVtable(Memory mem, SymbolTable st, Listing listing, Address vt) {
        try {
            for (int i = 0; i < 24; i++) {
                Address slot = vt.add(i * 8L);
                long ptr = mem.getLong(slot) & 0xffffffffffffffffL;
                if (ptr < 0x140000000L || ptr > 0x150000000L) break;
                Address fa = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(ptr);
                Function f = listing.getFunctionAt(fa);
                String nm = (f != null) ? f.getName(true) : "?";
                println(String.format("      vt[%2d] +0x%02x -> 0x%x  %s", i, i*8, ptr, nm));
            }
        } catch (Exception e) {
            println("      (vtable read error: " + e.getMessage() + ")");
        }
    }
}
