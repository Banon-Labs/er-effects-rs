import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.data.*;
import java.util.*;

// Resolve the ReleaseFlag gate (Unk55/Unk56), GetReleaseFlag, IsNotReleaseFlag55,
// callers of the offline-popup builder FUN_14082ff10, and the title-top builder.
public class AutoloadGate2 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;

    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }

    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG2] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG2] no func"); return; }
        println("[AG2] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG2] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void callers(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){ println("[AG2] callers:no func @0x"+Long.toHexString(va)); return; }
        println("[AG2] ==== CALLERS of "+tag+" "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0;
        while(ri.hasNext()){ Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[AG2]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>40){println("[AG2](cap)");break;} }
    }
    void symScan(String q){
        SymbolIterator it=st.getSymbolIterator(); int n=0;
        println("[AG2] ==== SYMS '"+q+"' ====");
        while(it.hasNext()){ Symbol s=it.next();
            if(s.getName().toLowerCase().contains(q.toLowerCase())){
                println("[AG2-SYM] "+s.getName()+" @ "+s.getAddress());
                n++; if(n>50){println("[AG2](symcap)");break;} } }
    }
    void enumDump(String q){
        DataTypeManager dtm=currentProgram.getDataTypeManager();
        Iterator<DataType> it=dtm.getAllDataTypes();
        while(it.hasNext()){ DataType dt=it.next();
            if(dt instanceof ghidra.program.model.data.Enum && dt.getName().toLowerCase().contains(q.toLowerCase())){
                ghidra.program.model.data.Enum e=(ghidra.program.model.data.Enum)dt;
                println("[AG2-ENUM] "+e.getPathName());
                for(long v: e.getValues()){ println("   "+e.getName(v)+" = "+v+" (0x"+Long.toHexString(v)+")"); }
            } }
    }

    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);

        // The offline-mode-popup builder
        decAt("offlinePopupBuilder_82ff10", 0x14082ff10L);
        callers("offlinePopupBuilder_82ff10", 0x14082ff10L);
        // The "clean" branch job built when flag55+flag56 both signal logged-in
        decAt("cleanBranch_7a7340", 0x1407a7340L);

        // ReleaseFlag machinery
        symScan("GetReleaseFlag");
        symScan("IsNotReleaseFlag55");
        symScan("ReleaseFlag");
        enumDump("ReleaseFlag");

        println("[AG2] DONE");
    }
}
