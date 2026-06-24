import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.*;
import java.util.*;

public class AutoloadGate5 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG5] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG5] no func"); return; }
        println("[AG5] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG5] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    // Find functions that reference the constant 0x18c (offset write) AND are near title flow.
    // Better: scan FUN_14082ff10's region for the TitleFlowContext singleton accessor.
    void callees(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){println("[AG5] callees:no func");return;}
        println("[AG5] ==== CALLEES from "+tag+" "+fname(f)+" ====");
        Set<Long> seen=new HashSet<>();
        InstructionIterator ii=currentProgram.getListing().getInstructions(f.getBody(),true);
        while(ii.hasNext()){ Instruction in=ii.next();
            for(Reference r: in.getReferencesFrom()){
                if(r.getReferenceType().isCall()){
                    Function cf=fm.getFunctionAt(r.getToAddress());
                    if(cf!=null && seen.add(cf.getEntryPoint().getOffset()))
                        println("[AG5]  call -> "+fname(cf)+" @0x"+Long.toHexString(cf.getEntryPoint().getOffset()));
                } } }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);

        // 14082c860 symbol named "TitleFlowContext" -- what is it?
        decAt("sym_TitleFlowContext_82c860", 0x14082c860L);
        // list callees of the offline-popup builder to find the singleton accessor + Menu_IsEnableOnlineMode
        callees("offlinePopupBuilder_82ff10", 0x14082ff10L);
        // The flag55 release check inlines Menu_IsEnableOnlineMode; decompile the case Unk55 source.
        decAt("GetReleaseFlag_765120", 0x140765120L);

        // Find WHO WRITES TitleFlowContext+0x18c: scan refs to functions whose body writes mov byte ptr [reg+0x18c]
        // Instead, decompile the title-flow step functions around 14082xxxx that set the field.
        // Caller chain: FUN_14082ecd0 -> FUN_14082dd60 -> FUN_14082ff10
        decAt("step_82ecd0", 0x14082ecd0L);
        println("[AG5] DONE");
    }
}
