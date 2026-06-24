import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Decompile the Tos dialog realize/show callers to find the agreement-accepted gate,
// and resolve vftable symbols by name.
public class TosCallerProbe extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[TCP] ==== DECOMP "+tag+" @0x"+Long.toHexString(va)+" ====");
        if(f==null){ println("[TCP] no func"); return; }
        println("[TCP] "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" "+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,120,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[TCP] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void callers(String tag,long va,int depthHint){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){ println("[TCP] callers:no func "+Long.toHexString(va)); return; }
        println("[TCP] ==== CALLERS of "+tag+" "+fname(f)+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0;
        while(ri.hasNext()){
            Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[TCP]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>30){println("[TCP](cap)");break;}
        }
    }
    void symByName(String q){
        println("[TCP] ==== SYMS matching '"+q+"' ====");
        SymbolIterator it=st.getSymbolIterator(); int n=0;
        while(it.hasNext()){
            Symbol s=it.next();
            if(s.getName().contains(q)){
                println("[TCP-SYM] "+s.getName()+" @ "+s.getAddress()+" ns="+(s.getParentNamespace()!=null?s.getParentNamespace().getName(true):"-"));
                n++; if(n>40){println("[TCP](symcap)");break;}
            }
        }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        println("[TCP] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        // vftable symbols
        symByName("TosDialog::vftable");
        symByName("TosMultiLangDialog::vftable");
        symByName("TosDialog");
        symByName("TosMultiLang");

        // realize/show callers (these likely host the gate)
        decAt("realize_14081e290", 0x14081e290L);
        decAt("realize_14081ea30", 0x14081ea30L);
        decAt("realize_1409b61c0", 0x1409b61c0L);
        callers("realize_14081e290", 0x14081e290L,1);
        callers("realize_14081ea30", 0x14081ea30L,1);
        println("[TCP] DONE");
    }
}
