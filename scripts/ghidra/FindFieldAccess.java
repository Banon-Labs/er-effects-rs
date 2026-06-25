// FindFieldAccess.java 0xGLOBAL 0xFIELDOFF
// Finds all functions that reference the global pointer 0xGLOBAL, then within each,
// scans for ANY instruction whose text references [reg + 0xFIELDOFF] (read OR write),
// printing the function, address, mnemonic, and a read/write hint.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import java.util.*;

public class FindFieldAccess extends GhidraScript {
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
        String needle = "+ 0x" + Long.toHexString(fieldOff) + "]";
        println("Scanning " + funcs.size() + " functions touching " + glob + " for [reg" + needle);
        for (Function f : funcs) {
            InstructionIterator ii = lst.getInstructions(f.getBody(), true);
            while (ii.hasNext()) {
                Instruction insn = ii.next();
                String txt = insn.toString();
                if (txt.contains(needle)) {
                    String rw = "?";
                    try {
                        RefType rt0 = insn.getOperandRefType(0);
                        RefType rt1 = insn.getNumOperands()>1?insn.getOperandRefType(1):null;
                        if (rt0 != null && (rt0.isWrite())) rw = "WRITE";
                        else if (rt1 != null && rt1.isRead()) rw = "READ";
                        else rw = "read/other";
                    } catch (Exception e) {}
                    println("  " + f.getName() + "@" + f.getEntryPoint() + "  " + insn.getAddress() + " : " + txt + "   [" + rw + "]");
                }
            }
        }
    }
}
