import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import java.util.*;

// Resolve the Tos dialog builders, their callers, the TosDialog/TosMultiLangDialog
// vtables (from RTTI type-descriptor -> COL -> vtable), and hunt the agreement-accepted
// gate flag.
public class TosGateProbe extends GhidraScript {
    DecompInterface dec; FunctionManager fm; AddressSpace sp; SymbolTable st; Memory mem;

    String fname(Function f){ if(f==null) return "<null>"; String n=f.getName();
        try{ n=(f.getParentNamespace()!=null?f.getParentNamespace().getName(true)+"::":"")+n; }catch(Exception e){} return n; }

    void decAt(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        println("[TGP] ==== DECOMP "+tag+" @0x"+Long.toHexString(va)+" ====");
        if(f==null){ println("[TGP] no func"); return; }
        println("[TGP] "+fname(f)+" @0x"+Long.toHexString(f.getEntryPoint().getOffset())+" "+f.getSignature());
        DecompileResults r=dec.decompileFunction(f,120,monitor);
        if(r!=null&&r.decompileCompleted()) println(r.getDecompiledFunction().getC());
        else println("[TGP] FAIL "+(r!=null?r.getErrorMessage():"null"));
    }

    void callers(String tag,long va){
        Function f=fm.getFunctionContaining(sp.getAddress(va));
        if(f==null){ println("[TGP] callers: no func @"+Long.toHexString(va)); return; }
        println("[TGP] ==== CALLERS of "+tag+" "+fname(f)+" ====");
        ReferenceIterator ri=currentProgram.getReferenceManager().getReferencesTo(f.getEntryPoint());
        int n=0;
        while(ri.hasNext()){
            Reference r=ri.next(); Address from=r.getFromAddress();
            Function cf=fm.getFunctionContaining(from);
            println("[TGP]  call from "+from+" ("+r.getReferenceType()+") in "+fname(cf)+(cf!=null?"@0x"+Long.toHexString(cf.getEntryPoint().getOffset()):""));
            n++; if(n>30){println("[TGP] (cap)");break;}
        }
        println("[TGP]  total callers="+n);
    }

    // RTTI type-descriptor VA -> find COL referencing it -> vtable (vtable is 8 bytes after COL ptr slot)
    void rttiVtable(String name,long typeDescVa){
        println("[TGP] ==== RTTI "+name+" typedesc=0x"+Long.toHexString(typeDescVa)+" ====");
        // scan .data/.rdata for 4-byte RVA references to typeDescVa-... actually COLs store
        // a 32-bit image-relative offset to the type descriptor. base:
        long base=currentProgram.getImageBase().getOffset();
        int wantRva=(int)(typeDescVa - base);
        // search initialized blocks for the 4-byte little-endian RVA
        for(MemoryBlock blk: mem.getBlocks()){
            if(!blk.isInitialized()) continue;
            if(blk.getName().startsWith(".text")) continue;
            long sz=blk.getSize(); if(sz>0x6000000) continue;
            byte[] buf=new byte[(int)sz];
            try{ blk.getBytes(blk.getStart(),buf);}catch(Exception e){continue;}
            for(int i=0;i+4<=buf.length;i++){
                int v=(buf[i]&0xff)|((buf[i+1]&0xff)<<8)|((buf[i+2]&0xff)<<16)|((buf[i+3]&0xff)<<24);
                if(v==wantRva){
                    Address colAddr=blk.getStart().add(i-12); // COL: sig,off,cdoff,typeDescRva(at+12)
                    // find pointers to COL (the vtable[-1] slot)
                    long colVa=colAddr.getOffset();
                    findPtrTo(name, colVa);
                }
            }
        }
    }
    void findPtrTo(String name,long colVa){
        for(MemoryBlock blk: mem.getBlocks()){
            if(!blk.isInitialized()) continue;
            if(blk.getName().startsWith(".text")) continue;
            long sz=blk.getSize(); if(sz>0x6000000) continue;
            byte[] buf=new byte[(int)sz];
            try{ blk.getBytes(blk.getStart(),buf);}catch(Exception e){continue;}
            for(int i=0;i+8<=buf.length;i++){
                long v=0; for(int j=0;j<8;j++) v|=((long)(buf[i+j]&0xff))<<(8*j);
                if(v==colVa){
                    long vtable=blk.getStart().add(i+8).getOffset();
                    println(String.format("[TGP-VT] %s COL@0x%x -> VTABLE 0x%x", name, colVa, vtable));
                }
            }
        }
    }

    public void run() throws Exception{
        fm=currentProgram.getFunctionManager();
        sp=currentProgram.getAddressFactory().getDefaultAddressSpace();
        st=currentProgram.getSymbolTable(); mem=currentProgram.getMemory();
        dec=new DecompInterface(); dec.openProgram(currentProgram);
        println("[TGP] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        decAt("builder_9b4350", 0x1409b4350L);
        decAt("builder_9b5ac0", 0x1409b5ac0L);
        callers("builder_9b4350", 0x1409b4350L);
        callers("builder_9b5ac0", 0x1409b5ac0L);

        rttiVtable("TosDialog", 0x143cdc0f0L);
        rttiVtable("TosMultiLangDialog", 0x143cdc788L);

        println("[TGP] DONE");
    }
}
