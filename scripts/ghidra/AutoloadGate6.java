import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.*;
import java.util.*;

// Find writers of TitleFlowContext+0x18c (notReleaseFlag55), the singleton instance,
// and Menu_IsEnableOnlineMode definition.
public class AutoloadGate6 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG6] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG6] no func"); return; }
        println("[AG6] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG6] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void symScan(String q){
        SymbolIterator it=st.getSymbolIterator(); int n=0;
        println("[AG6] ==== SYMS '"+q+"' ====");
        while(it.hasNext()){ Symbol s=it.next();
            if(s.getName().toLowerCase().contains(q.toLowerCase())){
                Function f=fm.getFunctionAt(s.getAddress());
                println("[AG6-SYM] "+s.getName()+" @ "+s.getAddress()+(f!=null?" [func]":""));
                n++; if(n>40){println("[AG6](symcap)");break;} } }
    }
    // Scan a set of title-flow functions for instructions that write byte/dword to [reg+0x18c]
    void scanWrites18c(long lo, long hi){
        println("[AG6] ==== scan stores to +0x18c in [0x"+Long.toHexString(lo)+",0x"+Long.toHexString(hi)+") ====");
        Listing lst=currentProgram.getListing();
        InstructionIterator ii=lst.getInstructions(sp.getAddress(lo),true);
        while(ii.hasNext()){ Instruction in=ii.next();
            long a=in.getAddress().getOffset(); if(a>=hi) break;
            String m=in.toString();
            if((m.startsWith("MOV")||m.startsWith("AND")||m.startsWith("OR")) && m.contains("0x18c")){
                Function f=fm.getFunctionContaining(in.getAddress());
                println("[AG6-W18c] 0x"+Long.toHexString(a)+"  "+m+"  in "+fname(f)+(f!=null?"@0x"+Long.toHexString(f.getEntryPoint().getOffset()):""));
            }
        }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);

        symScan("Menu_IsEnableOnlineMode");
        symScan("EnableOnlineMode");
        symScan("OnlineMode");
        // scan the title-flow code region for writes to +0x18c
        scanWrites18c(0x14082c000L, 0x14083b000L);

        // the MessageBoxDialog job builder
        decAt("msgboxJobBuilder_7b8260", 0x1407b8260L);
        println("[AG6] DONE");
    }
}
