// Decompile ONLY open_menu (dump 0x1409b2630, deobf 0x1409b24e0).
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;

public class OpenMenuOnly extends GhidraScript {
    @Override
    public void run() throws Exception {
        Address a = toAddr(0x1409b2630L);
        Function f = getFunctionContaining(a);
        println("VA(dump)=0x1409b2630 entry=0x" + f.getEntryPoint() + " name=" + f.getName());
        println("sig=" + f.getSignature());
        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        DecompileResults r = di.decompileFunction(f, 90, monitor);
        println("<<<BEGIN>>>");
        println(r.getDecompiledFunction().getC());
        println("<<<END>>>");
    }
}
