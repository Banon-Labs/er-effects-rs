// ScanDispWrite.java 0xSTART 0xEND 0xDISP [mode]
// Scan all instructions in [START,END) for memory operand with displacement 0xDISP.
// Prints the instruction, its function, and whether op0 is a write target.
// mode "write" (default) = only when displacement appears in operand 0 (store dest);
// mode "any" = any operand.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.RefType;

public class ScanDispWrite extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address start = currentProgram.getAddressFactory().getAddress(a[0]);
        Address end = currentProgram.getAddressFactory().getAddress(a[1]);
        long disp = Long.decode(a[2]);
        boolean anyOp = a.length > 3 && a[3].equals("any");
        Listing lst = currentProgram.getListing();
        FunctionManager fm = currentProgram.getFunctionManager();
        String needle1 = "+ 0x" + Long.toHexString(disp) + "]";
        String needle2 = "+ 0x" + Long.toHexString(disp) + ",";
        InstructionIterator ii = lst.getInstructions(start, true);
        int n = 0;
        while (ii.hasNext()) {
            Instruction insn = ii.next();
            if (insn.getAddress().compareTo(end) >= 0) break;
            String txt = insn.toString();
            if (!(txt.contains(needle1) || txt.contains(needle2))) continue;
            // For "write" mode, require the displacement-bearing operand to be operand 0 (dest).
            boolean op0 = false;
            try {
                String o0 = insn.getDefaultOperandRepresentation(0);
                if (o0.contains("0x"+Long.toHexString(disp))) op0 = true;
            } catch (Exception e) {}
            if (!anyOp && !op0) continue;
            Function f = fm.getFunctionContaining(insn.getAddress());
            println("  " + insn.getAddress() + " (" + (f!=null?f.getName():"?") + ") : " + txt
                + (op0?"  [op0/dest]":""));
            if (++n > 200) { println("  ...truncated"); break; }
        }
        if (n==0) println("  (no matches in range)");
    }
}
