//Adds labels to DL2 stepper functions by examining the initterm table for 
//their init functions.
//
//@author vswarte
//@category Dantelion

import java.util.HashMap;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.Instruction;

// Walks the CRT initialization list fed to _initterm looking for stepper inits
// It recognizes these by expecting any stepper init to call into memset.
public class StepperScraper extends GhidraScript {
    public void run() throws Exception {
        var inittermFnPtr = toAddr("_initterm");
        if (inittermFnPtr == null) {
            println("Could not locate _initterm. Make sure it's labeled.");
            return;
        }

        var table = extractInittermParameters(inittermFnPtr);
        if (table == null) {
            println("Could not locate the CRT table.");
            return;
        }

        var current = table.start;
        while (!current.equals(table.end) && !monitor.isCancelled()) {
            var fnPointerVal = getLong(current);
            if (fnPointerVal == 0x0) {
                current = current.add(0x8);
                continue;
            }

            var fnPointer = toAddr(fnPointerVal);
            if (isStepperInitCandidate(fnPointer)) {
                extractSteps(fnPointer).forEach((key, value) -> {
                    print(key + ", \"" + value + "\"\n");
                });
            } else {
                //println("Fn does not match stepper init pattern");
            }

            current = current.add(0x8);
            monitor.checkCanceled();
        }
    }

    // Extracts the CRT init list from calls to _initterm by examining calls 
    // to _initterm and looking for the one happening from 
    // __scrt_common_main_seh. Call pattern:
    // LEA RDX, [<CRT init table end>]
    // LEA RCX, [<CRT init table start>]
    // CALL _initterm
    private TableResult extractInittermParameters(Address fn) {
        // Grab first instruction in the _initterm in order to figure out the 
        // xrefs.
        var firstIns = getInstructionAt(fn);

        TableResult result = null;

        var xrefIter = firstIns.getReferenceIteratorTo();
        while (xrefIter.hasNext()) {
            var currentXref = xrefIter.next();
            var fromAddress = currentXref.getFromAddress();
            var callingFn = getFunctionContaining(fromAddress);
            if (callingFn == null ||
                !callingFn.getName().equals("__scrt_common_main_seh")) {
                continue;
            }

            var callIns = getInstructionAt(fromAddress);
            var tableStartIns = callIns.getPrevious();
            var tableEndIns = tableStartIns.getPrevious();

            var tableStart = getAddressOperand(tableStartIns, 1);
            var tableEnd = getAddressOperand(tableEndIns, 1);

            result = new TableResult(tableStart, tableEnd);
            break;
        }

        return result;
    }

    // Vets a fn pointer from the initterm table to see if it's a stepper init.
    // The pattern we're looking for:
    //
    // ```
    // SUB RSP, 0x28
    // XOR EDX, EDX
    // LEA RCX, [DAT_143cd66f0]
    // MOV R8D, <size of structure>
    // CALL memset
    // ```
    private boolean isStepperInitCandidate(Address start) {
        var subRspInstruction = getInstructionAt(start);
        if (subRspInstruction == null ||
            !subRspInstruction.toString().equals("SUB RSP,0x28")) {
            return false;
        }

        var xorInstruction = subRspInstruction.getNext();
        if (xorInstruction == null ||
            !xorInstruction.toString().equals("XOR EDX,EDX")) {
            return false;
        }

        var leaInstruction = xorInstruction.getNext();
        if (leaInstruction == null ||
            !leaInstruction.getMnemonicString().equals("LEA")) {
            return false;
        }

        var sizeParamInstruction = leaInstruction.getNext();
        if (sizeParamInstruction == null ||
            (
             !sizeParamInstruction.getMnemonicString().equals("LEA") &&
             !sizeParamInstruction.getMnemonicString().equals("MOV")
            )
        ){
            return false;
        }

        var callInstruction = sizeParamInstruction.getNext();
        if (callInstruction == null ||
            !callInstruction.getMnemonicString().equals("CALL")) {
            return false;
        }

        return true;
    }

    // Extract steps from the function by matching the following pattern:
    // LEA RAX, [<stepper function pointer>]
    // MOV [<other stepper function pointer>], RAX
    // LEA RAX, [<stepper function name>]
    // MOV [<other stepper function name>], RAX
    //
    // This pattern starts *after* the memset call and should continue until we 
    // meet a ADD RSP, 0x28.
    private HashMap<Address, String> extractSteps(Address start) {
        // Skip first five instructions since we don't need it and we've 
        // already validated these instructions per isStepperInitCandidate.
        // Kinda hacky but whatever...
        var currentBase = getInstructionAt(start)
            .getNext()
            .getNext()
            .getNext()
            .getNext()
            .getNext();

        var results = new HashMap<Address, String>();
        // Repeat until we meet the epilogue
        while (!currentBase.toString().equals("ADD RSP,0x28")) {
            var stepperFnPtrLea = currentBase;
            var namePtrLea = currentBase.getNext().getNext();

            var stepperFnPtr = getAddressOperand(stepperFnPtrLea, 1);
            var stepperNamePtr = getAddressOperand(namePtrLea, 1);

            var stepName = getDataAt(stepperNamePtr).getValue();
            results.put(stepperFnPtr, (String) stepName);

            currentBase = currentBase.getNext().getNext().getNext().getNext();
        }

        return results;
    }

    private Address getAddressOperand(Instruction ins, int operand) {
        return ins.getOperandReferences(operand)[0].getToAddress();
    }

    private class TableResult {
        public Address start;
        public Address end;

        public TableResult(Address start, Address end) {
            this.start = start;
            this.end = end;
        }
    }
}
