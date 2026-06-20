//Find DLRuntimeClass static object addresses
//@author Dasaav
//@category Analysis
//@keybinding 
//@menupath 
//@toolbar

import java.util.regex.Matcher;
import java.util.regex.Pattern;

import ghidra.app.script.GhidraScript;

import ghidra.util.task.*;

import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;

public class StaticDLRtClassFinder extends GhidraScript {
    private TaskMonitor taskMonitor;

    private Memory programMemory;
    private SymbolTable symbolTable;
    private ReferenceManager referenceManager;

    private long imageBase;

    private MemoryBlock dataSection;
    private MemoryBlock rdataSection;
    private MemoryBlock textSection;

    private Address dataSectionStart;
    private Address dataSectionEnd;

    private byte[] readbuf;
    private Pattern regex;

    public void run() throws Exception {
        init();
        this.taskMonitor.setMessage("Indexing .data section");
        this.taskMonitor.initialize(this.dataSection.getSize());
        walkDataSection();
    }

    private void init() {
        this.taskMonitor = monitor;
        this.programMemory = currentProgram.getMemory();
        this.symbolTable = currentProgram.getSymbolTable();
        this.referenceManager = currentProgram.getReferenceManager();
        this.imageBase = currentProgram.getImageBase().getOffset();
        this.dataSection = this.programMemory.getBlock(".data");
        this.rdataSection = this.programMemory.getBlock(".rdata");
        this.textSection = this.programMemory.getBlock(".text");
        this.dataSectionStart = this.dataSection.getStart();
        this.dataSectionEnd = this.dataSection.getEnd();
        this.readbuf = new byte[5];
        this.regex = Pattern.compile("^DLRuntimeClassImpl<class_FD4::FD4Singleton<.*?,class_(.*?)>.*>$");
    }

    private void walkDataSection() throws Exception {
        var first = this.dataSectionStart;
        var firstLong = first.getOffset();
        var lastLong = this.dataSectionEnd.getOffset();
        while (firstLong < lastLong) {
            this.taskMonitor.checkCanceled();
            try {
                var vtableAddress = this.getPointedToAddress(this.dataSection, first);
                var name = this.getDLRtClassname(first, vtableAddress);
                if (name != null) {
                    var magicByteAddress = getMagicByteAddress(vtableAddress);
                    var staticAddress = getStaticAddress(magicByteAddress);
                    if (staticAddress != null) {
                        printf("%s::instance,0x%X\n", name, staticAddress.getOffset() - this.imageBase);
                    }
                }
            }
            catch (Exception e) {}
            firstLong += 8;
            first = first.add(8);
            this.taskMonitor.incrementProgress(8);
        }
    }

    private String getDLRtClassname(Address address, Address vtableAddress) throws Exception {
        if (!this.rdataSection.contains(vtableAddress)) return null;
        var symbol = getSymbolAt(vtableAddress);
        if (symbol != null) {
            var namespace = symbol.getParentNamespace();
            if (namespace != null) {
                var name = namespace.getName();
                var matcher = this.regex.matcher(name);
                if (matcher.find()) {
                    return matcher.group(1);
                }
            }
        }
        // 11th vtable index:
        var methodAddress = getPointedToAddress(this.rdataSection, vtableAddress.add(88));
        if (!this.textSection.contains(methodAddress)) return null;
        symbol = getSymbolAt(methodAddress);
        if (symbol == null || !symbol.getName().equals("add_method_invoker")) return null;
        var cstrAddress = getPointedToAddress(this.dataSection, address.add(56));
        if (!this.rdataSection.contains(cstrAddress)) return null;
        var result = this.readTerminatedCString(this.rdataSection, cstrAddress);
        if (result == null) return null;
        if (result.startsWith("FD4")) {
            return "FD4::" + result;
        }
        return result;
    }

    private Address getMagicByteAddress(Address address) throws Exception {
        // 3rd vtable index:
        var out = address.add(24);
        out = getPointedToAddress(this.rdataSection, out);
        if (!this.textSection.contains(out)) return null;
        // LEA RAX,[DAT_14???????] offset to immediate 32-bit
        out = out.add(3);
        var intbuf = new byte[4];
        this.textSection.getBytes(out, intbuf);
        out = out.add((long)intbufToInt(intbuf) + 4);
        return this.dataSection.contains(out) ? out : null;
    }

    private Address getStaticAddress(Address address) throws Exception {
        var intbuf = new byte[4];
        var iter = this.referenceManager.getReferencesTo(address);
        for (var reference : iter) {
            var callAddress = reference.getFromAddress();
            callAddress = callAddress.add(35);
            this.textSection.getBytes(callAddress, intbuf);
            callAddress = callAddress.add(4);
            var symbolAddress = callAddress.add((long)intbufToInt(intbuf));
            if (!this.textSection.contains(symbolAddress)) continue;
            var symbol = getSymbolAt(symbolAddress);
            if (symbol == null || !symbol.getName().equals("DL2Panic")) continue;
            var staticAddress = callAddress.add(3);
            if (this.textSection.getByte(staticAddress) == 0x90) {
                // There is a NOP after the DL2Panic call
                staticAddress = staticAddress.add(1);
            }
            this.textSection.getBytes(staticAddress, intbuf);
            staticAddress = staticAddress.add((long)intbufToInt(intbuf) + 4);
            if (!this.dataSection.contains(staticAddress)) continue;
            return staticAddress;
        }
        return null;
    }
    
    private Address getPointedToAddress(MemoryBlock section, Address address) throws Exception {
        section.getBytes(address, this.readbuf);
        return toAddr((((long)this.readbuf[4] & 0xFF) << 32)
                    | (((long)this.readbuf[3] & 0xFF) << 24)
                    | (((long)this.readbuf[2] & 0xFF) << 16)
                    | (((long)this.readbuf[1] & 0xFF) << 8)
                    | (((long)this.readbuf[0] & 0xFF)));
    }

    private static int intbufToInt(byte[] intbuf) {
        return (((int)intbuf[3] & 0xFF) << 24)
             | (((int)intbuf[2] & 0xFF) << 16)
             | (((int)intbuf[1] & 0xFF) << 8)
             | (((int)intbuf[0] & 0xFF));
    }

    private String readTerminatedCString(MemoryBlock section, Address address) throws Exception {
        var first = address.add(0);
        var stringBuilder = new StringBuilder(64);
        for (int i = 0; i < 64; ++i) {
            this.taskMonitor.checkCanceled();
            byte b = section.getByte(first);
            if (b == 0) break;
            stringBuilder.appendCodePoint(b);
            first = first.add(1);
        }
        return stringBuilder.toString();
    }
}
