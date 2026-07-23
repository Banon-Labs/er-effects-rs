import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class TitleMenuDecomp3 extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label);
        println("VA(dump)=0x" + Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " entry=0x" + f.getEntryPoint() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 60, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("  FAIL " + (r==null?"null":r.getErrorMessage()));
    }
    void sym(long v, String label){
        Address a = toAddr(v);
        Symbol s = getSymbolAt(a);
        println(label + " 0x"+Long.toHexString(v)+" -> "+(s==null?"(no sym)":s.getName()));
    }
    @Override public void run() throws Exception {
        di = new DecompInterface();
        di.openProgram(currentProgram);
        dec(0x140b0c6a0L, "STEP_BeginTitle");
        dec(0x140b0d090L, "STEP_InitMenu");
        dec(0x140b0c630L, "STEP_BeginNewGame");
        // Identify the CSPS5Activity singleton object and what GetRuntimeClassName(&DAT_143d5df48) names.
        sym(0x143d5df48L, "DAT_143d5df48 (BeginLogo singleton rtti)");
        sym(0x143d5ae1dL, "DAT_143d5ae1d (CSMenuMan rtti)");
    }
}
