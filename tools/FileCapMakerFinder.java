//Find makeFileCap functions for FD4FileCap derived classes
//@author Dasaav
//@category Analysis
//@keybinding 
//@menupath 
//@toolbar

import java.util.*;

import ghidra.app.script.GhidraScript;

import ghidra.util.task.*;

import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;

public class FileCapMakerFinder extends GhidraScript {
    private TaskMonitor taskMonitor;

    private SymbolTable symbolTable;
    private ReferenceManager referenceManager;
    private FunctionManager functionManager;

    private long imageBase;
    
    private MemoryBlock textSection;

    public void run() throws Exception {
        init();
        this.taskMonitor.setMessage("Getting FD4FileCap vtables...");
        var fileCapVtables = this.getFileCapVtables();
        this.taskMonitor.setMessage("Finding potential FD4FileCap constructors...");
        this.taskMonitor.initialize(fileCapVtables.size());
        var ctors = this.findPotentialFileCapCtors(fileCapVtables);
        this.taskMonitor.setMessage("Confirming FD4FileCap constructors...");
        ctors = this.intersectFileCapCtors(ctors);
        this.taskMonitor.setMessage("Finding FileCap makers...");
        this.taskMonitor.initialize(ctors.size());
        this.findFileCapMakers(ctors);
    }

    private void init() {
        this.taskMonitor = monitor;
        this.symbolTable = currentProgram.getSymbolTable();
        this.referenceManager = currentProgram.getReferenceManager();
        this.functionManager = currentProgram.getFunctionManager();
        this.imageBase = currentProgram.getImageBase().getOffset();
        this.textSection = currentProgram.getMemory().getBlock(".text");
    }

    private ArrayList<Symbol> getFileCapVtables() throws Exception {
        var out = new ArrayList<Symbol>();
        for (var symbol : this.symbolTable.getSymbols("vftable")) {
            this.taskMonitor.checkCanceled();
            if (symbol.getParentNamespace().getName().endsWith("FileCap")) {
                out.add(symbol);
            }
        }
        return out;
    }

    private HashMap<Function, Symbol> findPotentialFileCapCtors(ArrayList<Symbol> vtables) throws Exception {
        var ctors = new HashMap<Function, Symbol>();
        for (var vtable : vtables) {
            this.taskMonitor.checkCanceled();
            for (var reference : vtable.getReferences(this.taskMonitor)) {
                var call = reference.getFromAddress();
                if (!this.textSection.contains(call)) continue;
                // There may be an EH reference that makes Ghidra
                // incorrectly split the function, so walk backwards first.
                var function = this.functionManager.getFunctionContaining(call.add(-56l));
                if (function == null) continue;
                ctors.put(function, vtable);
            }
            this.taskMonitor.incrementProgress(1);
        }
        return ctors;
    }

    private HashMap<Function, Symbol> intersectFileCapCtors(HashMap<Function, Symbol> potentialCtors) throws Exception {
        var allSymbols = this.symbolTable.getSymbols("FD4FileCap");
        Symbol ctorSymbol = null;
        while (allSymbols.hasNext()) {
            this.taskMonitor.checkCanceled();
            var next = allSymbols.next();
            if (next.getParentNamespace().getName(true).endsWith("FD4::FD4FileCap")) {
                if (ctorSymbol != null) {
                    throw new RuntimeException("Multiple FD4::FD4FileCap::FD4FileCap symbols defined, but only one constructor is expected");
                }
                ctorSymbol = next;
            }
        }
        if (ctorSymbol == null) {
            throw new RuntimeException("No FD4::FD4FileCap::FD4FileCap symbols are defined: define a FD4::FD4FileCap constructor");
        }
        var actualCtors = new HashSet<Function>();
        for (var caller : ctorSymbol.getReferences()) {
            this.taskMonitor.checkCanceled();
            var callAddress = caller.getFromAddress();
            if (!this.textSection.contains(callAddress)) continue;
            var function = this.functionManager.getFunctionContaining(callAddress);
            if (function == null) continue;
            actualCtors.add(function);
        }
        var resultCtors = new HashMap<Function, Symbol>();
        for (var ctor : actualCtors) {
            var vtable = potentialCtors.get(ctor);
            if (vtable == null) continue;
            resultCtors.put(ctor, vtable);
        }
        return resultCtors;
    }

    private void findFileCapMakers(HashMap<Function, Symbol> ctors) throws Exception {
        for (var ctor : ctors.keySet()) {
            this.taskMonitor.checkCanceled();
            for (var caller : ctor.getSymbol().getReferences()) {
                var callAddress = caller.getFromAddress();
                if (!this.textSection.contains(callAddress)) continue;
                var function = this.functionManager.getFunctionContaining(callAddress);
                if (function == null) continue;
                var namespace = ctors.get(ctor).getParentNamespace();
                if (namespace == null) continue;
                printf("CS::CSFile::make%s,0x%X\n", namespace.getName(false), function.getEntryPoint().getOffset() - this.imageBase);
            }
        }
    }
}
