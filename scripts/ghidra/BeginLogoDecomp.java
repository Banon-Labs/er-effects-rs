// Decompile STEP_BeginLogo (dump 0x140b0c390) and dump its called addresses.
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class BeginLogoDecomp extends GhidraScript {
    @Override
    public void run() throws Exception {
        Function f = getFunctionContaining(toAddr(0x140b0c390L));
        println("name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        DecompileResults r = di.decompileFunction(f, 120, monitor);
        println("<<<BEGIN>>>");
        println(r.getDecompiledFunction().getC());
        println("<<<END>>>");
        // list all CALL targets with names
        Listing lst = currentProgram.getListing();
        InstructionIterator it = lst.getInstructions(f.getBody(), true);
        println("---CALLS---");
        while (it.hasNext()) {
            Instruction ins = it.next();
            if (ins.getFlowType().isCall()) {
                Address[] flows = ins.getFlows();
                for (Address t : flows) {
                    Function tf = getFunctionContaining(t);
                    println("call@" + ins.getAddress() + " -> 0x" + t + (tf!=null?(" "+tf.getName()):""));
                }
            }
        }
    }
}
