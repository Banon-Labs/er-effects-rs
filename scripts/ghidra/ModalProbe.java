import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.data.*;

// Static RE probe for the startup MessageBoxDialog blocker on the title->Continue flow.
// Resolves function identities + decompiles for: the modal-build fn near 0x1407b0480,
// the MessageBoxDialog builder 0x1409275b0, the node-update 0x1407ad1c0, the OK-handler
// chain (0x14078ef20 -> 0x1407a91e0 / 0x1407a9200), and resolves the data pointers used
// to build the dialog. Dump is ~0x10 below the live/deobf VA, so probe a small offset window.
public class ModalProbe extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;
    AddressSpace sp;

    String fname(Function f) {
        if (f == null) return "<null>";
        String nm = f.getName();
        try { nm = (f.getParentNamespace()!=null? f.getParentNamespace().getName(true)+"::":"") + nm; } catch (Exception e) {}
        return nm;
    }

    void info(String tag, long va) {
        for (long off : new long[]{0, -0x10, -0x11, +0x10, +0x11, -0x20, +0x20}) {
            Address a = sp.getAddress(va+off);
            Function f = fm.getFunctionContaining(a);
            if (f != null) {
                println(String.format("[MP] %s va=0x%x (off %+d) -> %s @0x%x sig=%s",
                    tag, va, off, fname(f), f.getEntryPoint().getOffset(), f.getSignature()));
                return;
            }
        }
        println(String.format("[MP] %s va=0x%x -> NO FUNC (any offset)", tag, va));
    }

    void dec(String tag, long va) {
        Function f = null;
        for (long off : new long[]{0, -0x10, -0x11, +0x10, +0x11, -0x20, +0x20}) {
            f = fm.getFunctionContaining(sp.getAddress(va+off));
            if (f != null) break;
        }
        println("[MP] ===== DECOMP "+tag+" containing 0x"+Long.toHexString(va)+" =====");
        if (f == null) { println("[MP] NO FUNC"); return; }
        println("[MP] name="+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" sig="+f.getSignature());
        DecompileResults r = dec.decompileFunction(f, 120, monitor);
        if (r != null && r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[MP] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }

    void data(String tag, long va) {
        Address a = sp.getAddress(va);
        Symbol[] syms = currentProgram.getSymbolTable().getSymbols(a);
        StringBuilder sb = new StringBuilder();
        for (Symbol s: syms) sb.append(s.getName()).append(" ");
        Data d = getDataAt(a);
        String dval = (d!=null? d.getDataType().getName()+" = "+d.getDefaultValueRepresentation() : "<no data>");
        println(String.format("[MP] DATA %s 0x%x : syms=[%s] %s", tag, va, sb.toString().trim(), dval));
        try {
            long p = currentProgram.getMemory().getLong(a);
            Address pa = sp.getAddress(p);
            Symbol[] psyms = currentProgram.getSymbolTable().getSymbols(pa);
            StringBuilder psb = new StringBuilder();
            for (Symbol s: psyms) psb.append(s.getName()).append(" ");
            Function pf = fm.getFunctionContaining(pa);
            println(String.format("[MP]   *0x%x = 0x%x  psyms=[%s] func=%s", va, p, psb.toString().trim(), fname(pf)));
        } catch (Exception e) { println("[MP]   (deref failed: "+e+")"); }
    }

    public void run() throws Exception {
        fm = currentProgram.getFunctionManager();
        sp = currentProgram.getAddressFactory().getDefaultAddressSpace();
        dec = new DecompInterface();
        dec.openProgram(currentProgram);

        println("[MP] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        info("modal_build_fn", 0x1407b0480L);
        info("builder_msgbox", 0x1409275b0L);
        info("node_update", 0x1407ad1c0L);
        info("ok_handler", 0x14078e030L);
        info("ok_chain_ef20", 0x14078ef20L);
        info("pred_a9200", 0x1407a9200L);
        info("result_a91e0", 0x1407a91e0L);
        info("open_menu", 0x1409b24e0L);

        data("alloc_global_d87350", 0x143d87350L);
        data("rdi_str_9e2848", 0x1429e2848L);
        data("vt_aaad90", 0x142aaad90L);
        data("vt_aaadc8", 0x142aaadc8L);
        data("g_d83148", 0x143d83148L);

        dec("modal_build_fn", 0x1407b0480L);
        dec("builder_msgbox", 0x1409275b0L);
        dec("ok_chain_ef20", 0x14078ef20L);
        dec("result_a91e0", 0x1407a91e0L);
        dec("pred_a9200", 0x1407a9200L);

        println("[MP] DONE");
    }
}
