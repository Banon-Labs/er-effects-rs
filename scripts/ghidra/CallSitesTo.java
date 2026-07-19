// CallSitesTo.java <0xADDR> [0xADDR ...]
// Robustly enumerate call sites of a target function even when callers reference a thunk.
// 1) resolve the function containing ADDR, report thunk info
// 2) collect the set of "entry" addresses: the body entry + any thunk entries that thunk to it
// 3) getReferencesTo each of those
// 4) ALSO scan all instructions for a flow reference (call/jump) whose target is the body entry
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.*;

public class CallSitesTo extends GhidraScript {
    @Override
    public void run() throws Exception {
        FunctionManager fm = currentProgram.getFunctionManager();
        ReferenceManager rm = currentProgram.getReferenceManager();
        for (String a : getScriptArgs()) {
            Address addr = currentProgram.getAddressFactory().getAddress(a);
            Function f = fm.getFunctionContaining(addr);
            Address body = (f != null) ? f.getEntryPoint() : addr;
            println("\n==== target " + a + " -> " + (f != null ? f.getName() + " @ " + body : "(no func)") + " ====");
            if (f != null) println("  isThunk=" + f.isThunk() + " thunked=" + f.getThunkedFunction(true));

            LinkedHashSet<Address> entries = new LinkedHashSet<>();
            entries.add(body);
            // find thunk functions that point at body
            FunctionIterator fi = fm.getFunctions(true);
            while (fi.hasNext()) {
                Function g = fi.next();
                if (g.isThunk()) {
                    Function t = g.getThunkedFunction(true);
                    if (t != null && t.getEntryPoint().equals(body)) {
                        entries.add(g.getEntryPoint());
                        println("  thunk: " + g.getName() + " @ " + g.getEntryPoint());
                    }
                }
            }

            LinkedHashSet<Function> callers = new LinkedHashSet<>();
            for (Address e : entries) {
                ReferenceIterator it = rm.getReferencesTo(e);
                while (it.hasNext()) {
                    Reference r = it.next();
                    Function cf = fm.getFunctionContaining(r.getFromAddress());
                    println("  ref-> " + e + " from " + r.getFromAddress()
                            + " (" + (cf != null ? cf.getName() + "@" + cf.getEntryPoint() : "?")
                            + ") " + r.getReferenceType());
                    if (cf != null) callers.add(cf);
                }
            }
            println("  distinct caller funcs: " + callers.size());
            for (Function c : callers) println("    * " + c.getName() + " @ " + c.getEntryPoint());
        }
    }
}
