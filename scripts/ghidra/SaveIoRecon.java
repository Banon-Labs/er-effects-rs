import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.data.*;
import java.util.*;

// Save-IO reconnaissance:
//  (1) Find wide/ascii string literals: ER0000.sl2, ER0000.co2, .sl2, .bak, EldenRing,
//      Roaming, AppData, steam_autocloud.vdf, %s, USER_DATA  -> print addr + xrefs + containing fn.
//  (2) Decompile the folder builder at dump 0x140e0e730 (deobf 0x140e0e680).
//  (3) Resolve CreateFileW / CreateFile2 / CreateFileA / NtCreateFile imports and list ALL call sites.
//  (4) Walk callers of the known deserialize/slot fns to find the file-open layer.
public class SaveIoRecon extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;
    AddressSpace sp;
    ReferenceManager rm;

    String decompName(Address a) {
        Function f = fm.getFunctionContaining(a);
        return f != null ? (f.getName(true) + "@0x" + Long.toHexString(f.getEntryPoint().getOffset())) : "?";
    }

    public void run() throws Exception {
        fm = currentProgram.getFunctionManager();
        sp = currentProgram.getAddressFactory().getDefaultAddressSpace();
        rm = currentProgram.getReferenceManager();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);
        Listing listing = currentProgram.getListing();

        String[] needles = {
            "ER0000", ".sl2", ".co2", ".bak", "EldenRing", "Roaming", "AppData",
            "steam_autocloud", "USER_DATA", "%s\\%s", "\\EldenRing\\"
        };

        println("===== (1) STRING LITERALS =====");
        DataIterator dit = listing.getDefinedData(true);
        int printed = 0;
        while (dit.hasNext()) {
            Data d = dit.next();
            if (d == null) continue;
            Object v = d.getValue();
            if (!(v instanceof String)) continue;
            String s = (String) v;
            String low = s.toLowerCase();
            boolean hit = false;
            for (String n : needles) { if (low.contains(n.toLowerCase())) { hit = true; break; } }
            if (!hit) continue;
            Address da = d.getAddress();
            // xrefs to this string
            StringBuilder xr = new StringBuilder();
            ReferenceIterator it = rm.getReferencesTo(da);
            int xc = 0;
            while (it.hasNext() && xc < 12) {
                Reference r = it.next();
                xr.append(String.format(" <-0x%x[%s]", r.getFromAddress().getOffset(), decompName(r.getFromAddress())));
                xc++;
            }
            println(String.format("STR 0x%x type=%s %-40s%s", da.getOffset(), d.getDataType().getName(),
                    "\"" + (s.length() > 40 ? s.substring(0,40) : s) + "\"", xr.toString()));
            printed++;
            if (printed > 200) { println("...truncated string list..."); break; }
        }

        println("\n===== (2) FOLDER BUILDER decompile (dump 0x140e0e730) =====");
        decompAt(0x140e0e730L);

        println("\n===== (3) FILE-OPEN IMPORTS =====");
        String[] apis = {"CreateFileW", "CreateFile2", "CreateFileA", "NtCreateFile", "NtOpenFile", "ReadFile", "WriteFile"};
        SymbolTable st = currentProgram.getSymbolTable();
        for (String api : apis) {
            SymbolIterator sit = st.getSymbols(api);
            boolean any = false;
            while (sit.hasNext()) {
                Symbol sym = sit.next();
                any = true;
                Address symAddr = sym.getAddress();
                println(String.format("IMPORT %s @0x%x (%s)", api, symAddr.getOffset(), sym.getSymbolType()));
                // call sites
                ReferenceIterator it = rm.getReferencesTo(symAddr);
                int cc = 0;
                while (it.hasNext()) {
                    Reference r = it.next();
                    Address from = r.getFromAddress();
                    println(String.format("   call/ref 0x%x in %s", from.getOffset(), decompName(from)));
                    cc++;
                    if (cc > 60) { println("   ...truncated call sites..."); break; }
                }
            }
            if (!any) println("IMPORT " + api + ": NOT FOUND as named symbol");
        }

        println("\n===== (4) CALLERS of known save fns =====");
        // deobf VAs from prompt; convert roughly by scanning dump near (we just report callers in dump space).
        long[] knownDump = { 0x14067b1a0L, 0x14067b290L, 0x14067b100L, 0x14067b750L, 0x14067a810L, 0x1406798d0L, 0x14067bd70L };
        for (long g : knownDump) {
            Function f = fm.getFunctionContaining(sp.getAddress(g));
            if (f == null) { println("known 0x" + Long.toHexString(g) + " -> no fn (try window)");
                for (long dd=-0x40; dd<=0x40 && f==null; dd+=4) f = fm.getFunctionContaining(sp.getAddress(g+dd));
            }
            if (f == null) { println("  still no fn for 0x"+Long.toHexString(g)); continue; }
            println(String.format("known ~0x%x -> %s @0x%x", g, f.getName(true), f.getEntryPoint().getOffset()));
            ReferenceIterator it = rm.getReferencesTo(f.getEntryPoint());
            int cc = 0;
            while (it.hasNext() && cc < 25) {
                Reference r = it.next();
                if (!r.getReferenceType().isCall() && !r.getReferenceType().isJump()) { continue; }
                println("    caller 0x" + Long.toHexString(r.getFromAddress().getOffset()) + " in " + decompName(r.getFromAddress()));
                cc++;
            }
        }
        println("\n[DONE]");
    }

    void decompAt(long va) {
        Function f = fm.getFunctionContaining(sp.getAddress(va));
        if (f == null) { println("no function at 0x" + Long.toHexString(va)); return; }
        println("// fn " + f.getName(true) + " @0x" + Long.toHexString(f.getEntryPoint().getOffset()));
        DecompileResults res = dec.decompileFunction(f, 120, monitor);
        if (res != null && res.decompileCompleted())
            println(res.getDecompiledFunction().getC());
        else
            println("decompile FAILED: " + (res != null ? res.getErrorMessage() : "null"));
    }
}
