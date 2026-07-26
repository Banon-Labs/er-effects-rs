import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RtSaveDrive extends GhidraScript {
    @Override public void run() throws Exception {
        Listing lst = currentProgram.getListing();
        // 1) disasm FUN_140aff730 (orchestrator caller; decompile came back empty)
        println("################ DISASM FUN_140aff730 (first 50) ################");
        Address a = toAddr(0x140aff730L);
        for (int i = 0; i < 50; i++) {
            Instruction insn = lst.getInstructionAt(a);
            if (insn == null) { println("  (no instruction at " + a + ")"); break; }
            println("  " + insn.getAddress() + "  " + insn);
            if (insn.getMnemonicString().toLowerCase().startsWith("ret")) break;
            a = insn.getAddress().add(insn.getLength());
        }
        // 2) any data/vtable references TO 0x140aff730 (which vtable holds this step method)
        println("################ ALL refs to 140aff730 (vtable slots) ################");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(0x140aff730L));
        while (it.hasNext()) {
            Reference r = it.next();
            println("  from " + r.getFromAddress() + "  type=" + r.getReferenceType());
        }
        // 3) decompile the save-manager per-frame update 0x14067f5d0
        DecompInterface di = new DecompInterface(); di.setOptions(new DecompileOptions()); di.openProgram(currentProgram);
        Function f = getFunctionContaining(toAddr(0x14067f5d0L));
        println("################ DECOMP 0x14067f5d0 -> " + (f!=null?f.getName():"?") + " ################");
        if (f != null) {
            DecompileResults r = di.decompileFunction(f, 150, monitor);
            if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
            else println("(decompile failed)");
        }
    }
}
