import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

public class AutoloadGate3 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG3] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG3] no func"); return; }
        println("[AG3] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG3] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void callers(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){ println("[AG3] callers:no func"); return; }
        println("[AG3] ==== CALLERS of "+tag+" "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0; while(ri.hasNext()){ Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[AG3]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>40){println("[AG3](cap)");break;} }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        decAt("GetReleaseFlag_765120", 0x140765120L);
        decAt("IsNotReleaseFlag55_82ce50", 0x14082ce50L);
        decAt("caller_82dd60", 0x14082dd60L);
        callers("caller_82dd60", 0x14082dd60L);
        println("[AG3] DONE");
    }
}
