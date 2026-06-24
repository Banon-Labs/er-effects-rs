// NameAddrs: given a list of DUMP VAs (hex 0x... args), print the containing function name,
// entry point, and signature for each. Used to symbolize boot-profiler RIP hotspots after the
// deobf->dump shift mapping. Run via: bash scripts/ghidra-query.sh scripts/ghidra/NameAddrs.java 0x.. 0x..
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Symbol;

public class NameAddrs extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        for (String a : args) {
            long va;
            try {
                va = Long.decode(a);
            } catch (NumberFormatException e) {
                println("ADDR " + a + " -> BAD_ARG");
                continue;
            }
            Address addr = toAddr(va);
            if (addr == null) {
                println("ADDR " + a + " -> NULL_ADDR");
                continue;
            }
            Function f = getFunctionContaining(addr);
            if (f != null) {
                long off = addr.getOffset() - f.getEntryPoint().getOffset();
                println("ADDR " + a + " -> " + f.getName() + "+0x" + Long.toHexString(off)
                        + " entry=" + f.getEntryPoint() + " sig=" + f.getSignature().getPrototypeString());
            } else {
                Symbol s = getSymbolAt(addr);
                println("ADDR " + a + " -> NO_FUNC" + (s != null ? " sym=" + s.getName() : ""));
            }
        }
    }
}
