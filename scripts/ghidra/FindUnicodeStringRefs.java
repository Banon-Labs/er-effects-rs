import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;

public class FindUnicodeStringRefs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length == 0) {
            println("usage: FindUnicodeStringRefs <ascii-substring>");
            return;
        }
        String needle = args[0];
        byte[] pat = new byte[needle.length() * 2];
        for (int i = 0; i < needle.length(); i++) {
            char c = needle.charAt(i);
            pat[i * 2] = (byte)(c & 0xff);
            pat[i * 2 + 1] = (byte)((c >> 8) & 0xff);
        }
        Memory mem = currentProgram.getMemory();
        Address cur = currentProgram.getMinAddress();
        int found = 0;
        while (cur != null) {
            Address a = mem.findBytes(cur, pat, null, true, monitor);
            if (a == null) break;
            println("STRING " + needle + " at " + a);
            Reference[] refs = getReferencesTo(a);
            for (Reference r : refs) {
                Address from = r.getFromAddress();
                Function f = getFunctionContaining(from);
                println("  REF from " + from + (f == null ? " NO_FUNC" : " func=" + f.getName() + " entry=" + f.getEntryPoint()));
            }
            found++;
            cur = a.add(1);
            if (found >= 50) break;
        }
        println("FOUND " + found);
    }
}
