import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
public class RtRefsToData extends GhidraScript {
    @Override public void run() throws Exception {
        for (String a : getScriptArgs()) {
            Address addr = toAddr(Long.decode(a));
            println("################ refs TO " + addr + " ################");
            ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(addr);
            int n=0;
            while (it.hasNext()) {
                Reference r = it.next();
                Function f = getFunctionContaining(r.getFromAddress());
                println("  " + r.getFromAddress() + "  " + (f!=null?f.getName()+" @ "+f.getEntryPoint():"?") + "  " + r.getReferenceType());
                if(++n>30){println("  ...");break;}
            }
            if(n==0) println("  (none)");
        }
    }
}
