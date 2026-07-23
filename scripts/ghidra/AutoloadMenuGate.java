import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import java.util.*;

// Decompile the menu-open -> GR_System_Message -> MessageBoxDialog path.
// All VAs below are DUMP VAs (this project == the runtime dump).
public class AutoloadMenuGate extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;

    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }

    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AMG] ==================== DECOMP "+tag+" @0x"+Long.toHexString(va)+" ====================");
        if(f==null){ println("[AMG] no func at 0x"+Long.toHexString(va)); return; }
        println("[AMG] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AMG] DECOMP FAIL "+(r!=null?r.getErrorMessage():"null"));
    }

    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        println("[AMG] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        String[] args=getScriptArgs();
        if(args.length>0){
            for(String a:args){ long va=Long.decode(a); decAt(a,va); }
            println("[AMG] DONE-ARGS"); return;
        }

        // Requesters (dump VAs)
        decAt("req401110_83acac", 0x14083ad9cL);
        decAt("req401170_msgbox_83004d", 0x1408301bdL);
        // MessageBox-build call chain
        decAt("mbcaller_7b04c7", 0x1407b05b7L);
        decAt("mbcaller_7ad2bc", 0x1407ad3acL);
        decAt("mbcaller_7aa2bb", 0x1407aa3abL);
        decAt("mbcaller_7a747c", 0x1407a756cL);
        decAt("mbcaller_793346", 0x140793436L);
        println("[AMG] DONE");
    }
}
