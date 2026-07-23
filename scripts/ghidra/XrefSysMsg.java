import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

public class XrefSysMsg extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        List<Function> targets = new ArrayList<>();
        FunctionManager fm = currentProgram.getFunctionManager();
        SymbolTable st = currentProgram.getSymbolTable();
        if (args.length > 0 && args[0].startsWith("0x")) {
            Address a = currentProgram.getAddressFactory().getAddress(args[0]);
            Function f = fm.getFunctionContaining(a);
            if (f != null) targets.add(f);
            println("ADDR " + args[0] + " -> func " + (f!=null?f.getName()+" @ "+f.getEntryPoint():"<none>"));
        }
        String nameQuery = (args.length > 0 && !args[0].startsWith("0x")) ? args[0] : "GetGR_System_Message";
        SymbolIterator it = st.getSymbolIterator(nameQuery, true);
        while (it.hasNext()) {
            Symbol s = it.next();
            Function f = fm.getFunctionAt(s.getAddress());
            if (f == null) f = fm.getFunctionContaining(s.getAddress());
            println("SYMBOL " + s.getName() + " @ " + s.getAddress() + (f!=null?" func="+f.getName()+"@"+f.getEntryPoint():""));
            if (f != null && !targets.contains(f)) targets.add(f);
        }
        FunctionIterator fit = fm.getFunctions(true);
        while (fit.hasNext()) {
            Function f = fit.next();
            if (f.getName().contains("GR_System_Message") && !targets.contains(f)) {
                targets.add(f);
                println("NAMEMATCH " + f.getName() + " @ " + f.getEntryPoint());
            }
        }
        for (Function tf : targets) {
            println("=== XREFS TO " + tf.getName() + " @ " + tf.getEntryPoint() + " ===");
            ReferenceManager rm = currentProgram.getReferenceManager();
            ReferenceIterator ri = rm.getReferencesTo(tf.getEntryPoint());
            int n = 0;
            while (ri.hasNext()) {
                Reference r = ri.next();
                Address from = r.getFromAddress();
                Function cf = fm.getFunctionContaining(from);
                println("  XREF from " + from + " (" + r.getReferenceType() + ")" + (cf!=null?" in "+cf.getName()+"@"+cf.getEntryPoint():""));
                n++;
            }
            println("  total xrefs: " + n);
        }
    }
}
