// FindFieldWrite.java <baseRegHintIgnored> -- actually:
// Usage: FindFieldWrite.java 0xGLOBAL  0xFIELDOFF
// 1) Finds all functions that READ the global pointer 0xGLOBAL.
// 2) Within each such function, scans instructions for a store to [reg + FIELDOFF]
//    (mov byte/word/dword ptr [reg+off], ...) where off == FIELDOFF, and prints them.
// Also: generic mode "scanwrite 0xFIELDOFF 0xSTART 0xEND" not implemented; keep simple.
// This catches "load global into reg; ... mov [reg+0x21], imm".
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.scalar.Scalar;
import java.util.*;

public class FindFieldWrite extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        Address glob = currentProgram.getAddressFactory().getAddress(args[0]);
        long fieldOff = Long.decode(args[1]);
        ReferenceManager rm = currentProgram.getReferenceManager();
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing lst = currentProgram.getListing();

        LinkedHashSet<Function> funcs = new LinkedHashSet<>();
        ReferenceIterator it = rm.getReferencesTo(glob);
        while (it.hasNext()) {
            Reference r = it.next();
            Function f = fm.getFunctionContaining(r.getFromAddress());
            if (f != null) funcs.add(f);
        }
        println("Scanning " + funcs.size() + " functions that touch " + glob + " for writes to [reg+0x" + Long.toHexString(fieldOff) + "]");
        for (Function f : funcs) {
            InstructionIterator ii = lst.getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction insn = ii.next();
                String m = insn.getMnemonicString().toLowerCase();
                if (!m.equals("mov") && !m.equals("and") && !m.equals("or") && !m.equals("xor")) continue;
                // operand 0 should be a memory operand with displacement == fieldOff
                int nOps = insn.getNumOperands();
                if (nOps < 1) continue;
                if (insn.getOperandRefType(0) != ghidra.program.model.symbol.RefType.WRITE
                    && insn.getOperandRefType(0) != ghidra.program.model.symbol.RefType.READ_WRITE) {
                    // displacement-based store may not be flagged; still inspect text
                }
                String txt = insn.toString();
                // match "[REG + 0x21]" displacement
                String needle = "+ 0x" + Long.toHexString(fieldOff) + "]";
                String needle2 = "+ 0x" + Long.toHexString(fieldOff) + ",";
                if (txt.contains(needle) || txt.contains(needle2)) {
                    println("  " + f.getName() + " @ " + insn.getAddress() + " : " + txt);
                }
            }
        }
    }
}
