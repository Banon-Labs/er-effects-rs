// ScanFieldRead.java <fieldOffHex> [maxHits]
// Whole-program scan for instructions that reference [reg + fieldOff] as a SOURCE (read),
// i.e. the needle appears after the first comma (AT&T: src is 2nd; Ghidra listing uses Intel-ish
// "mnem dst, src" ordering in toString). Prints containing function + instruction.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;

public class ScanFieldRead extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        long fieldOff = Long.decode(args[0]);
        int maxHits = args.length > 1 ? Integer.parseInt(args[1]) : 200;
        String needle = "0x" + Long.toHexString(fieldOff) + "]";
        Listing lst = currentProgram.getListing();
        InstructionIterator ii;
        if (args.length > 3) {
            Address s = currentProgram.getAddressFactory().getAddress(args[2]);
            Address e = currentProgram.getAddressFactory().getAddress(args[3]);
            ghidra.program.model.address.AddressSet set =
                new ghidra.program.model.address.AddressSet(s, e);
            ii = lst.getInstructions(set, true);
        } else {
            ii = lst.getInstructions(true);
        }
        int hits = 0;
        while (ii.hasNext() && hits < maxHits) {
            Instruction insn = ii.next();
            String txt = insn.toString();
            int npos = txt.indexOf(needle);
            if (npos < 0) continue;
            // ensure the matched offset is exactly fieldOff (avoid 0x2f90 matching 0x2f9)
            // needle already ends with ']' so 0x2f9] won't match inside 0x2f90]. Good.
            int comma = txt.indexOf(',');
            boolean isSource = (comma >= 0 && npos > comma);
            Function f = getFunctionContaining(insn.getAddress());
            println((isSource ? "R " : "W ") + (f != null ? f.getName() + "@" + f.getEntryPoint() : "NOFUNC")
                    + "  " + insn.getAddress() + "  " + txt);
            hits++;
        }
        println("total=" + hits);
    }
}
