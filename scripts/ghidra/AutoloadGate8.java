import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Confirm the offline-popup step (FUN_14082ff10) is on the TitleTop/open_menu chain,
// locate the TitleFlowContext singleton, and dump the online-mode global symbol.
public class AutoloadGate8 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG8] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG8] no func"); return; }
        println("[AG8] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG8] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void symAt(long va){
        Symbol[] ss=st.getSymbols(sp.getAddress(va));
        println("[AG8] SYMS @0x"+Long.toHexString(va)+":");
        for(Symbol s: ss) println("   "+s.getName()+" ("+s.getSymbolType()+")");
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        // TitleTopDialog constructor (calls the ctx init FUN_14082d1c0)
        decAt("TitleTopDialog_ctor_9a82d0", 0x1409a82d0L);
        // FUN_1409b3050 (near open_menu) also calls the ctx init
        decAt("FUN_9b3050", 0x1409b3050L);
        // globals
        symAt(0x144588afcL);
        symAt(0x144588b00L);
        println("[AG8] DONE");
    }
}
