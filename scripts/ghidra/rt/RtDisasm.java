import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class RtDisasm extends GhidraScript {
    @Override public void run() throws Exception {
        Listing lst = currentProgram.getListing();
        for (String a : getScriptArgs()) {
            long va = Long.decode(a);
            Function f = getFunctionContaining(toAddr(va));
            println("################ " + a + " -> " + (f != null ? f.getName() : "?") + " ################");
            Address addr = f != null ? f.getEntryPoint() : toAddr(va);
            for (int i = 0; i < 30; i++) {
                Instruction insn = lst.getInstructionAt(addr);
                if (insn == null) break;
                println("  " + insn.getAddress() + "  " + insn.toString());
                if (insn.getMnemonicString().toLowerCase().startsWith("ret")) break;
                addr = insn.getAddress().add(insn.getLength());
            }
        }
    }
}
