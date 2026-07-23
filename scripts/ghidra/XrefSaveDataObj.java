import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import java.util.*;

// Resolve the save-data subsystem object [0x143d68078] (gate for 0x67bd70's c30 write)
// and save-load boot 0x6798d0. Goal: learn WHEN [0x143d68078] is built -- at game boot
// (independent of the FSM load we bypass) or only inside the FSM load path.
// Dump is address-shifted ~0x10-0x11 from the deobf binary, so scan a small window.
public class XrefSaveDataObj extends GhidraScript {
    public void run() throws Exception {
        FunctionManager fm = currentProgram.getFunctionManager();
        AddressSpace sp = currentProgram.getAddressFactory().getDefaultAddressSpace();
        ReferenceManager rm = currentProgram.getReferenceManager();
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);

        // (1) Resolve 0x6798d0 within a +-0x20 window for the dump shift.
        long bootGuess = 0x1406798d0L;
        println("[X] === resolve save-load boot near 0x" + Long.toHexString(bootGuess) + " ===");
        for (long d = -0x20; d <= 0x20; d += 4) {
            Function f = fm.getFunctionContaining(sp.getAddress(bootGuess + d));
            if (f != null) {
                println(String.format("[X] 0x%x (delta %d) -> %s @0x%x", bootGuess + d,
                        d, f.getName(true), f.getEntryPoint().getOffset()));
                break;
            }
        }

        // (2) References to the data object [0x143d68078] across a +-0x10 window.
        long objGuess = 0x143d68078L;
        println("[X] === refs to save-data obj near 0x" + Long.toHexString(objGuess) + " ===");
        Set<Function> writers = new LinkedHashSet<>();
        for (long d = -0x10; d <= 0x10; d += 8) {
            Address a = sp.getAddress(objGuess + d);
            ReferenceIterator it = rm.getReferencesTo(a);
            while (it.hasNext()) {
                Reference r = it.next();
                Address from = r.getFromAddress();
                Function cf = fm.getFunctionContaining(from);
                String cfn = cf != null ? cf.getName(true) : "?";
                boolean w = r.getReferenceType().isWrite();
                println(String.format("[X] obj+0x%x %s from 0x%x in %s%s", d,
                        r.getReferenceType().getName(), from.getOffset(), cfn,
                        w ? "  <== WRITE" : ""));
                if (w && cf != null) writers.add(cf);
            }
        }

        // (3) Decompile the writer(s) so we can see the build context/conditions.
        for (Function f : writers) {
            println("[X] ============ WRITER decompile: " + f.getName(true) + " @0x"
                    + Long.toHexString(f.getEntryPoint().getOffset()) + " ============");
            DecompileResults res = dec.decompileFunction(f, 120, monitor);
            if (res != null && res.decompileCompleted())
                println(res.getDecompiledFunction().getC());
            else
                println("[X] decompile FAILED: " + (res != null ? res.getErrorMessage() : "null"));
        }
        println("[X] DONE");
    }
}
