// ScanByteFieldWrite.java <fieldOffHex> [<immHex or ANY>]
// Whole-program scan for stores to [reg + fieldOff] (byte/word/dword mov/and/or), optionally
// filtered by an immediate value. Prints containing function + instruction. Used to find the
// world-block load-state lifecycle writers (+0x2c request, +0x2d ready, +0x35 phase==0xa).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class ScanByteFieldWrite extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        long fieldOff = Long.decode(args[0]);
        String imm = args.length > 1 ? args[1] : "ANY";
        String needle = "+ 0x" + Long.toHexString(fieldOff) + "]";
        Listing lst = currentProgram.getListing();
        InstructionIterator ii = lst.getInstructions(true);
        int hits = 0;
        while (ii.hasNext() && hits < 800) {
            Instruction insn = ii.next();
            String m = insn.getMnemonicString().toLowerCase();
            if (!m.equals("mov") && !m.equals("and") && !m.equals("or") && !m.equals("xor")) continue;
            String txt = insn.toString();
            if (!txt.contains(needle)) continue;
            // operand 0 must be the memory write (dest), i.e. needle appears before the comma
            int comma = txt.indexOf(',');
            int npos = txt.indexOf(needle);
            if (comma >= 0 && npos > comma) continue; // needle is the source, skip
            if (!imm.equals("ANY")) {
                if (!txt.toLowerCase().contains(imm.toLowerCase())) continue;
            }
            Function f = getFunctionContaining(insn.getAddress());
            println((f != null ? f.getName() + "@" + f.getEntryPoint() : "NOFUNC")
                    + "  " + insn.getAddress() + "  " + txt);
            hits++;
        }
        println("total=" + hits);
    }
}
