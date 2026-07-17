import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class RtCallers extends GhidraScript {
    @Override public void run() throws Exception {
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            Function tgt = getFunctionContaining(addr);
            println("################ refs TO " + a + " (" + (tgt!=null?tgt.getName():"?") + ") ################");
            ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(addr);
            int n=0;
            while (it.hasNext()) {
                Reference r = it.next();
                RefType rt = r.getReferenceType();
                Function f = getFunctionContaining(r.getFromAddress());
                println("  " + r.getFromAddress() + "  " + (f!=null?f.getName()+" @ "+f.getEntryPoint():"?") + "  " + rt);
                n++;
                if(n>40) { println("  ...(truncated)"); break; }
            }
            if(n==0) println("  (no refs)");
        }
    }
}
