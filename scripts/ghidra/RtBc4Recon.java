// RtBc4Recon: verify the return-title / quit-save mechanism in the 1.16.1 dump.
// Args: dump VAs to decompile (e.g. 0x14067a490 = the native return-title REQUEST FUN).
// Also: (a) lists named symbols for GameMan/CSMenuMan/menuData/return-title, and
// (b) scans every instruction for stores to [reg+0xbc4] (GameMan return-title predicate;
//     REQUEST writes 1, the quit-save pump advances it to 2 then 3).
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.*;

public class RtBc4Recon extends GhidraScript {
    @Override public void run() throws Exception {
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);

        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function f = getFunctionContaining(addr);
            println("=== DECOMP " + a + " -> " + (f != null ? f.getName() + " @ " + f.getEntryPoint() : "NO_FUNC") + " ===");
            if (f != null) {
                DecompileResults r = di.decompileFunction(f, 90, monitor);
                if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
                else println("(decompile failed)");
            }
        }

        println("=== NAMED SYMBOLS (gameman/menuman/csmenu/menudata/returntitle/finalfunctor/quit) ===");
        String[] kw = {"gameman","menuman","csmenu","menudata","returntitle","return_title","finalfunctor","final_functor","quitgame","reloadtitle","worldreset"};
        SymbolIterator si = currentProgram.getSymbolTable().getAllSymbols(true);
        int shown = 0;
        while (si.hasNext() && shown < 150) {
            Symbol s = si.next(); String n = s.getName().toLowerCase();
            for (String k : kw) if (n.contains(k)) { println("  " + s.getAddress() + "  " + s.getSymbolType() + "  " + s.getName()); shown++; break; }
        }

        println("=== WRITES to [reg+0xbc4] (bc4 predicate: 1=REQUEST, 2/3=pump) ===");
        Listing lst = currentProgram.getListing();
        InstructionIterator ii = lst.getInstructions(true);
        long count = 0; int hits = 0;
        while (ii.hasNext()) {
            Instruction insn = ii.next(); count++;
            String txt = insn.toString();
            if (txt.contains("0xbc4]")) {
                Function f = getFunctionContaining(insn.getAddress());
                println("  " + insn.getAddress() + " [" + (f != null ? f.getName() : "?") + "] " + txt);
                hits++;
            }
        }
        println("(scanned " + count + " instructions, " + hits + " bc4-offset hits)");
    }
}
