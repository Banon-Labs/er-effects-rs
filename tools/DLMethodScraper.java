//Searchs for recoverable function names using a pattern in DLMethodInvoker 
//usage.
//
//@author vswarte
//@category Dantelion

import java.util.regex.Matcher;
import java.util.regex.Pattern;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import ghidra.util.task.*;
import ghidra.program.model.data.PointerDataType;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class DLMethodScraper extends GhidraScript {
    private static final String PATTERN =
        "01001... 10001011 00010000 " +
        "01001... 10001101 00001101 ........ ........ ........ ........ " +
        "01001... 10001101 00000101 ........ ........ ........ ........ " +
        "01001... 10001101 00010101 ........ ........ ........ ........ " +
        "01001... 10001011 11001000 " +
        "01000... 11111111 01010010 01011000 ";

    public void run() throws Exception {
        var output = "Pointer,Function name,Class,Return type,Parameter types,Invoker name\n";

        var memory = currentProgram.getMemory();
        var textSection = memory.getBlock(".text");
        var typeDataRegex = Pattern.compile("const .*?<class (.*?),(.*?),(.*?)(?:,\\d)?>::vftable");
        var paramTypeRegex = Pattern.compile("struct DLUT::TypeList::DLTypeList<(.*?),");

        var occurrences = PatternFinder.findAllBitPattern(
            memory,
            textSection.getStart(),
            textSection.getEnd(),
            PATTERN,
            monitor
        );

        for (Address occurrence : occurrences) {
            var instanceLea = getInstructionAt(occurrence.add(17));
	    if (instanceLea == null) { continue; }

            var instancePtr = instanceLea.getOperandReferences(1)[0].getToAddress();

            var vmtPointer = getDataAt(instancePtr).getValue();

            var fnPointerPointer = instancePtr.add(8);
            var fnPointerData = getDataAt(fnPointerPointer);
            if (fnPointerData == null) {
                // TODO: Add comment clarifying that this script placed the def?
                createData(fnPointerPointer, new PointerDataType());
                println(
                    "Could not find a defined pointer for instance. Creating...: " +
                    fnPointerPointer
                );
                fnPointerData = getDataAt(fnPointerPointer);
            }

            var nameLea = getInstructionAt(occurrence.add(10));
            var namePtr = nameLea.getOperandReferences(1)[0].getToAddress();
            var nameData = getDataAt(namePtr);
            if (nameData == null) {
                println(
                    "Could not find a defined string for the fn name: " +
                    namePtr
                );
                continue;
            }

            var classForMethod = "";
            var returnType = "";
            var paramTypes = "";

            // It'll be fine dont worry...
            var className = getPlateComment(toAddr(vmtPointer.toString()));
            if (className == null) {
                println("Could not find classname for vmt: " + vmtPointer);
                //continue;
            } else {
                // TODO: probably a more ergonomic API for this
                var matcher = typeDataRegex.matcher(className);
                if (matcher.find()) {
                    classForMethod = matcher.group(1);
                    returnType = matcher.group(2);

                    var params = matcher.group(3);
                    var paramsMatcher = paramTypeRegex.matcher(params);

                    while (paramsMatcher.find()) {
                        paramTypes += paramsMatcher.group(1) + ",";
                    }
                } else {
                    println("Could not parse class name: " + className);
                }
            }

	    output += classForMethod + "::" + nameData.getValue() + " " + fnPointerData.getValue() + " f\n";

            //output +=
            //    fnPointerData.getValue() + ",\"" +
            //    nameData.getValue() + "\",\"" +
            //    classForMethod + "\",\"" +
            //    returnType + "\",\"" +
            //    paramTypes + "\",\"" +
            //    className + "\"\n";
        }

        println(output);

        //println("Found occurrences " + occurrences.size() + " in total");
    }

    static class PatternFinder {
        public static List<Address> findAllBitPattern(
            Memory memory,
            Address start,
            Address end,
            String bitPattern,
            TaskMonitor monitor
        ) {
            var pattern = parseBitPattern(bitPattern);
            var result = new ArrayList<Address>();

            var currentAddress = start;
            while (true) {
                var found = memory.findBytes(currentAddress, end, pattern[0], pattern[1], true, monitor);
                if (found == null) {
                    break;
                }
                result.add(found);
                currentAddress = found.add(1);
            }
            return result;
        }

        // Most of this came from mrexodia's impl and I'm too fucking lazy to 
        // clean it up.
        private static byte[][] parseBitPattern(String bitPattern) throws IllegalArgumentException {
            var mask = new ArrayList<Byte>();
            var pattern = new ArrayList<Byte>();

            // Split it all up by spaces
            var split = bitPattern.split(" ");

            // Cycle over all chunks
            for (var i = 0; i < split.length; i++) {
                var s = split[i];
                if (s.length() != 8) {
                    throw new IllegalArgumentException(String.format("Invalid length %d for pattern[%d] = '%s'", s.length(), i, s));
                }

                byte patternByte = 0;
                byte maskByte = 0;
                for (var j = 0; j < s.length(); j++) {
                    var patternBit = 0;
                    int maskBit;
                    var ch = s.charAt(j);
                    if (ch == '.') {
                        maskBit = 0;
                    } else if (ch == '0' || ch == '1') {
                        patternBit = ch == '1' ? 1 : 0;
                        maskBit = 1;
                    } else {
                        throw new IllegalArgumentException(String.format("Invalid character '%c' in pattern[%d] = '%s'", ch, i, s));
                    }
                    patternByte <<= 1;
                    patternByte |= patternBit;
                    maskByte <<= 1;
                    maskByte |= maskBit;
                }
                pattern.add(patternByte);
                mask.add(maskByte);
            }

            return new byte[][]{toByteArray(pattern), toByteArray(mask)};
        }

        private static byte[] toByteArray(ArrayList<Byte> in) {
            final int n = in.size();
            byte ret[] = new byte[n];
            for (int i = 0; i < n; i++) {
                ret[i] = in.get(i);
            }
            return ret;
        }
    }
}
