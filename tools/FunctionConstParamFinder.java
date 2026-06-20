//Place at the first instruction of a function and run. Will give you a table with all found constants used as function parameters.
//@author vswarte
//@category Functions
//@menupath Tools.Function.Get Constant Parameters

import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.pcode.PcodeOp;
import ghidra.program.model.pcode.Varnode;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.address.Address;
import ghidra.app.tablechooser.*;
import ghidra.util.*;

import java.util.ArrayList;
import java.util.regex.Pattern;

public class FunctionConstParamFinder extends GhidraScript {
    private static final int MAX_LOOKBACK = 20;
    private TableChooserDialog tableDialog;

    public void run() throws Exception {
        var entryAddress = currentLocation.getAddress();
        var function = getFunctionAt(entryAddress);
        if (function == null) {
            println("Cursor is not at a function start!");
            return;
        }

        // Figure out what varnodes we need to watch and build a colum header list
        var tableColumns = new ArrayList<String>();
        var watch = new ArrayList<String>();
        for (Parameter parameter : function.getParameters()) {
            var columnName = cleanStringForExport(parameter.getName()) + " (" + parameter.getVariableStorage() + ")";

            var register = parameter.getVariableStorage().getRegister();
            if (register != null) {
                var address = register.getAddress();
                var varnode = address.toString();
                println(" - " + parameter.getName() + " -> " + varnode);
                tableColumns.add(columnName);
                watch.add(varnode);
            }
        }

        TableChooserExecutor executor = null;
        tableDialog = createTableChooserDialog("Constant Function Arguments", executor);
        FunctionInvoke.buildTable(currentProgram, tableDialog, tableColumns);
        tableDialog.show();
        tableDialog.setMessage("Crawling for const parameter assignment...");

        var entryInstruction = getInstructionAt(entryAddress);
        for (Reference incomingReference : entryInstruction.getReferenceIteratorTo()) {
            var fromAddress = incomingReference.getFromAddress();
            var referringInstruction = getInstructionAt(fromAddress);
            if (referringInstruction == null) {
                println("Found non instruction ref to function: " + fromAddress);
                continue;
            }

            var results = new ArrayList<String>();
            for (var varnode : watch) {
                results.add(this.inferConstRegisterValue(referringInstruction, varnode));
            }

            tableDialog.add(new FunctionInvoke(
                getCurrentProgram(),
                fromAddress,
                results
            ));
        }

        tableDialog.setMessage("");
    }


    private String cleanStringForExport(String input) {
        var pattern = Pattern.compile("^(ds|unicode u)", Pattern.MULTILINE);
        var matcher = pattern.matcher(input.replace("\"", ""));
        return matcher.replaceFirst("");
    }

    private String inferConstRegisterValue(Instruction start, String register) {
        var populationOpcode = getRegisterPopulate(start, register);
        if (populationOpcode == null) {
            return "No population opcode";
        }

        var varnodeAddress = populationOpcode.getInput(0).getAddress();
        // HACKY
        if (varnodeAddress.toString().startsWith("const:14")) {
            // Hacky but required to obtain data types from general RAM memory space
            var address = toAddr(varnodeAddress.toString().replace("const:",""));
            if (getInstructionAt(address) != null) {
                return "Instruction: " + address;
            }

            var data = getDataAt(address);
            if (data == null) {
                println("No data defined at " + address);
                return "Undefined data";
            }

            return data.toString();
        } else {
            return varnodeAddress.toString().replace("const:","");
        }
    }

    private PcodeOp getRegisterPopulate(Instruction callInstruction, String register) {
        // Go up from CALL instruction
        var currentInstruction = this.getInstructionBeforeIncludingJmpsFrom(callInstruction);

        for (int i = 0; i < MAX_LOOKBACK; i++) {
            // TODO: reverse opcode stream so we can apply more intelligent register tracking
            for (PcodeOp pcodeOp : currentInstruction.getPcode()) {
                // Yeet everything that isn't touching the register
                if (pcodeOp.getOutput() == null || !this.isRegister(pcodeOp.getOutput(), register)) {
                    continue;
                }

                // Explicitly wiped
                if (pcodeOp.getMnemonic() == "INT_XOR" && this.isRegister(pcodeOp.getOutput(), register)) {
                    return null;
                }

                // Yeet everything that doesn't have a constant input
                if (pcodeOp.getInput(0) == null || !pcodeOp.getInput(0).isConstant()) {
                    continue;
                }

                return pcodeOp;
            }

            // Cycle to instruction before
            currentInstruction = this.getInstructionBeforeIncludingJmpsFrom(currentInstruction);
        }

        return null;
    }

    private Instruction getInstructionBeforeIncludingJmpsFrom(Instruction instruction) {
        var referencingIns = this.getJmpedFromInstruction(instruction);
        return referencingIns != null
                ? referencingIns
                : instruction.getPrevious();
    }

    // Checks if an instruction is landed on by unconditional jmps (usually indicates arxan)
    private Instruction getJmpedFromInstruction(Instruction instruction) {
        for (Reference incomingReference : instruction.getReferenceIteratorTo()) {
            var referenceAddress = incomingReference.getFromAddress();
            var referencingInstruction = getInstructionAt(referenceAddress);

            if (referencingInstruction == null) {
                continue;
            }

            if (this.hasBranchingOpcode(referencingInstruction)) {
                return referencingInstruction;
            }
        }

        return null;
    }

    private boolean isRegister(Varnode varnode, String register) {
        return varnode.getAddress().equals(toAddr(register));
    }

    private boolean hasBranchingOpcode(Instruction instruction) {
        for (PcodeOp pcodeOp : instruction.getPcode()) {
            switch (pcodeOp.getMnemonic()) {
                case "BRANCH":
                case "CBRANCH":
                case "BRANCHIND":
                case "CALL":
                case "CALLIND":
                case "RETURN":
                    return true;
                default:
                break;
            }
        }
        return false;
    }

    class FunctionInvoke implements AddressableRowObject {
        private Address addr;
        private ArrayList<String> values;

        FunctionInvoke(Program prog, Address addr, ArrayList<String> values) {
            this.addr = addr;
            this.values = values;
        }

        @Override
        public Address getAddress() {
            return addr;
        }

        public String getValue(int index) {
            return values.get(index);
        }

        public static void buildTable(Program currentProgram, TableChooserDialog dialog, ArrayList<String> parameterColumns) {
            StringColumnDisplay funcColumn = new StringColumnDisplay() {
                @Override
                public String getColumnName() {
                    return "Function Name";
                }

                @Override
                public String getColumnValue(AddressableRowObject rowObject) {
                    FunctionInvoke entry = (FunctionInvoke) rowObject;
                    Function func = currentProgram.getFunctionManager().getFunctionContaining(entry.getAddress());
                    if (func == null) {
                        return "";
                    }
                    return func.getName();
                }
            };

            dialog.addCustomColumn(funcColumn);

            for (var parameterColumn : parameterColumns) {
                StringColumnDisplay valueColumn = new StringColumnDisplay() {
                    @Override
                    public String getColumnName() {
                        return parameterColumn;
                    }

                    @Override
                    public String getColumnValue(AddressableRowObject rowObject) {
                        FunctionInvoke entry = (FunctionInvoke) rowObject;
                        return entry.getValue(parameterColumns.indexOf(parameterColumn));
                    }
                };

                dialog.addCustomColumn(valueColumn);
            }
        }
    }
}
