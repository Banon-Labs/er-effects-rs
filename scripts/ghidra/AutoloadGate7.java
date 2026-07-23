import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Decompile the writers of notReleaseFlag55 (+0x18c) and the online-mode getter.
public class AutoloadGate7 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG7] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG7] no func"); return; }
        println("[AG7] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG7] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void callers(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){println("[AG7] callers:no func");return;}
        println("[AG7] ==== CALLERS of "+tag+" "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0; while(ri.hasNext()){ Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[AG7]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>40){println("[AG7](cap)");break;} }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        decAt("writer_setto1_837500", 0x140837500L);
        callers("writer_setto1_837500", 0x140837500L);
        decAt("writer_var_82d1c0", 0x14082d1c0L);
        callers("writer_var_82d1c0", 0x14082d1c0L);
        decAt("Menu_IsEnableOnlineMode_e563c0", 0x140e563c0L);
        println("[AG7] DONE");
    }
}
