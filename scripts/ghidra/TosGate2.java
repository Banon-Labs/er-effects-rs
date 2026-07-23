import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Walk up from the Tos menu-open wrappers to find the gate that decides show vs skip.
public class TosGate2 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[TG2] ==== DECOMP "+tag+" @0x"+Long.toHexString(va)+" ====");
        if(f==null){ println("[TG2] no func"); return; }
        println("[TG2] "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" "+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,120,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[TG2] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void callers(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){ println("[TG2] callers:no func"); return; }
        println("[TG2] ==== CALLERS "+tag+" "+fname(f)+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0;
        while(ri.hasNext()){
            Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[TG2]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>40){println("[TG2](cap)");break;}
        }
    }
    void symByName(String q){
        SymbolIterator it=st.getSymbolIterator(); int n=0;
        println("[TG2] ==== SYMS '"+q+"' ====");
        while(it.hasNext()){ Symbol s=it.next();
            if(s.getName().toLowerCase().contains(q.toLowerCase())){
                println("[TG2-SYM] "+s.getName()+" @ "+s.getAddress());
                n++; if(n>40){println("[TG2](symcap)");break;}
            }
        }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        println("[TG2] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        decAt("openwrap_140820af0", 0x140820af0L);
        decAt("openwrap_140820d40", 0x140820d40L);
        callers("openwrap_140820af0", 0x140820af0L);
        callers("openwrap_140820d40", 0x140820d40L);

        // hunt globals that look like the accepted/agreed flag
        symByName("Agree");
        symByName("Accepted");
        symByName("Eula");
        symByName("ShowTos");
        symByName("TosState");
        symByName("CSSystemStep::STEP_Init_forBootPhase");
        println("[TG2] DONE");
    }
}
