// Resolve TitleFlowContext->regulationVersion offset & CSRegulationManager->regulationVersion;
// find callers of orchestrator FUN_14082f850; decompile the save-update job lambda invoke
// (the Run that performs the write & selects the corrupted-vs-updating message).
// Usage: ghidra-query.sh scripts/ghidra/SaveCtx.java
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.data.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class SaveCtx extends GhidraScript {
    DecompInterface di;
    void dec(long v, String label) throws Exception {
        Address a = toAddr(v);
        Function f = getFunctionContaining(a);
        println("==================================================== " + label + " 0x"+Long.toHexString(v));
        if (f == null) { println("  NO FUNCTION"); return; }
        println("  name=" + f.getName() + " sig=" + f.getSignature());
        DecompileResults r = di.decompileFunction(f, 200, monitor);
        if (r == null || !r.decompileCompleted()) { println("  FAILED"); return; }
        DecompiledFunction df = r.getDecompiledFunction();
        if (df != null) println(df.getC());
    }
    void struct(String name) {
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        java.util.Iterator<DataType> it = dtm.getAllDataTypes();
        while (it.hasNext()) {
            DataType d = it.next();
            if (d instanceof Structure && d.getName().equals(name)) {
                Structure s = (Structure)d;
                println("---- STRUCT " + name + " size=0x"+Integer.toHexString(s.getLength())+" ----");
                for (DataTypeComponent c : s.getComponents()) {
                    String fn = c.getFieldName();
                    if (fn != null && (fn.toLowerCase().contains("regul")||fn.toLowerCase().contains("version")||fn.toLowerCase().contains("step")))
                        println("  +0x"+Integer.toHexString(c.getOffset())+" "+c.getDataType().getName()+" "+fn);
                }
            }
        }
    }
    void xrefs(long v, String label) {
        println("---- XREFS TO " + label + " 0x" + Long.toHexString(v) + " ----");
        ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(v));
        while (it.hasNext()) {
            Reference rf = it.next();
            Function ff = getFunctionContaining(rf.getFromAddress());
            println("  from 0x" + rf.getFromAddress() + " (" + (ff==null?"?":ff.getName()) + ") " + rf.getReferenceType());
        }
    }
    @Override
    public void run() throws Exception {
        di = new DecompInterface();
        di.toggleCCode(true);
        di.openProgram(currentProgram);
        struct("TitleFlowContext");
        struct("CSRegulationManagerImp");
        xrefs(0x14082f850L, "ORCHESTRATOR FUN_14082f850");
        // the lambda body referenced by the Func_impl vftable in the factory.
        // factory passed &ppuStack_110 (a _Func_impl) into ctor; the invoke ptr is in that vftable.
        // Decompile the job-ctor's first real worker FUN_140821ba0 (builds the job from desc).
        dec(0x140821ba0L, "JOB BUILD WORKER FUN_140821ba0");
    }
}
