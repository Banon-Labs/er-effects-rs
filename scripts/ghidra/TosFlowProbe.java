import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Decompile the title/EULA/Tos flow functions and find xrefs to the s_Tos* /
// EulaLang binding labels, to locate the agreement-gate and dialog-realize code.
public class TosFlowProbe extends GhidraScript {
    DecompInterface dec;
    FunctionManager fm;
    AddressSpace sp;
    SymbolTable st;

    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }

    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[TFP] ==== DECOMP "+tag+" @0x"+Long.toHexString(va)+" ====");
        if(f==null){ println("[TFP] no func"); return; }
        println("[TFP] "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" "+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,120,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[TFP] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }

    void xrefsTo(String tag,long va){
        Address a=sp.getAddress(va);
        println("[TFP] ==== XREFS TO "+tag+" 0x"+Long.toHexString(va)+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(a);
        int n=0;
        while(ri.hasNext()){
            Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[TFP]  from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>40) { println("[TFP]  ...(cap)"); break; }
        }
        println("[TFP]  total="+n);
    }

    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        println("[TFP] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        // xrefs to the Tos/Eula binding string labels (who reads them)
        xrefsTo("s_TosText", 0x142b27328L);
        xrefsTo("s_TosTitle_Text", 0x142b27330L);
        xrefsTo("s_TosTitle", 0x142b281a8L);
        xrefsTo("Localize.EulaLang_label", 0x143b400b8L);

        // decompile the named flow functions
        decAt("GetEulaLangBySellRegion", 0x140e0f330L);
        decAt("TitleFlow.EnableTosDebug", 0x140e4fe50L);
        decAt("Localize.EulaLang", 0x140e5b4c0L);

        // also: find any function whose name contains Title/Boot/Flow to map the title state machine
        println("[TFP] ==== name-scan Title/Boot/Flow/Agree functions ====");
        FunctionIterator fit=fm.getFunctions(true); int c=0;
        while(fit.hasNext()){
            Function f=fit.next(); String n=f.getName();
            if(n.contains("Title")||n.contains("Eula")||n.contains("Tos")||n.contains("Agree")||n.contains("Boot")||n.contains("Flow")){
                println("[TFP-FN] "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" "+f.getSignature());
                c++; if(c>120){ println("[TFP] (fn cap)"); break; }
            }
        }
        println("[TFP] DONE");
    }
}
