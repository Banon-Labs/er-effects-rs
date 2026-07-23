//Uses strings involved in getting an event flag to determine getters caller's 
//name. Has a fixed pointer that has to be updated across games!
//
//@author Chainfailure
//@category Dantelion

import java.util.HashMap;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Instruction;

public class EventFlagMethodNameScraper extends GhidraScript {
    public void run() throws Exception {
        var paramFnPtr = toAddr("1405d7f60");

        var firstIns = getInstructionAt(paramFnPtr);

        var results = new HashMap<Address, String>();
        var xrefIter = firstIns.getReferenceIteratorTo();
        while (xrefIter.hasNext()) {
            var currentXref = xrefIter.next();
            var callAddress = currentXref.getFromAddress();
            var callingFn = getFunctionContaining(callAddress);
            if (callingFn == null) {
                println("WARN: Caller was not in a function. Please define a function for " + callAddress);
                continue;
            }

            var name = findNameString(callAddress);
            results.put(callingFn.getEntryPoint(), (String) name);
        }

        results.forEach((key, value) -> {
            print(key + ", \"" + value + "\"\n");
        });
    }

    private String findNameString(Address call) {
        var callIns = getInstructionAt(call);

        String result = null;
        var currentIns = callIns.getPrevious();
        for (var i = 0; i < 5; i++) {
            if (currentIns.toString().startsWith("LEA RDX")) {
                var stringPtr = getAddressOperand(currentIns, 1);
                return (String) getDataAt(stringPtr).getValue();
            }

            // Prevent us from accidentally getting the RDX param from a 
            // previous call.
            if (
                currentIns.getMnemonicString().equals("CALL") ||
                currentIns.getMnemonicString().equals("JMP")
            ) {
                break;
            }

            currentIns = currentIns.getPrevious();
        }

        return null;
    }

    private Address getAddressOperand(Instruction ins, int operand) {
        return ins.getOperandReferences(operand)[0].getToAddress();
    }
}
