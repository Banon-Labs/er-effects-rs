import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.data.*;
import java.util.*;

public class AutoloadGate4 extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st;
    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }
    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[AG4] ============ DECOMP "+tag+" @0x"+Long.toHexString(va)+" ============");
        if(f==null){ println("[AG4] no func"); return; }
        println("[AG4] FUNC "+fname(f)+" entry=0x"+Long.toHexString(f.getEntryPoint().getOffset())+"  sig="+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,180,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[AG4] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }
    void structDump(String q){
        DataTypeManager dtm=currentProgram.getDataTypeManager();
        Iterator<DataType> it=dtm.getAllDataTypes();
        while(it.hasNext()){ DataType dt=it.next();
            if(dt instanceof Structure && dt.getName().toLowerCase().contains(q.toLowerCase())){
                Structure s=(Structure)dt;
                println("[AG4-STRUCT] "+s.getPathName()+" len=0x"+Integer.toHexString(s.getLength()));
                for(DataTypeComponent c: s.getComponents()){
                    String nm=c.getFieldName();
                    if(nm!=null && (nm.toLowerCase().contains("flag")||nm.toLowerCase().contains("release")||nm.toLowerCase().contains("online")||nm.toLowerCase().contains("offline")))
                        println("   +0x"+Integer.toHexString(c.getOffset())+" "+c.getDataType().getName()+" "+nm);
                }
            } }
    }
    void symScan(String q){
        SymbolIterator it=st.getSymbolIterator(); int n=0;
        println("[AG4] ==== SYMS '"+q+"' ====");
        while(it.hasNext()){ Symbol s=it.next();
            if(s.getName().toLowerCase().contains(q.toLowerCase())){
                println("[AG4-SYM] "+s.getName()+" @ "+s.getAddress());
                n++; if(n>40){println("[AG4](symcap)");break;} } }
    }
    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        decAt("Menu_IsEnableOnlineMode", 0x0L); // filled below by symbol
        symScan("Menu_IsEnableOnlineMode");
        symScan("IsNetworkTest");
        symScan("TitleFlowContext");
        structDump("TitleFlowContext");
        // dump full TitleFlowContext struct fields near the flag
        DataTypeManager dtm=currentProgram.getDataTypeManager();
        Iterator<DataType> it=dtm.getAllDataTypes();
        while(it.hasNext()){ DataType dt=it.next();
            if(dt instanceof Structure && dt.getName().equals("TitleFlowContext")){
                Structure s=(Structure)dt;
                println("[AG4-FULL] TitleFlowContext len=0x"+Integer.toHexString(s.getLength()));
                for(DataTypeComponent c: s.getComponents()){
                    println("   +0x"+Integer.toHexString(c.getOffset())+" "+c.getDataType().getName()+" "+c.getFieldName());
                }
            } }
        println("[AG4] DONE");
    }
}
