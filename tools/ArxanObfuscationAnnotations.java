//Matches a bunch of patterns to find instances of JMP obfuscation
//@author Chainfailure
//@category Deobfuscation

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import ghidra.util.task.*;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class ArxanObfuscationAnnotations extends GhidraScript {
    public void run() {
        monitor.setMaximum(PATTERNS.length);

        var memory = currentProgram.getMemory();
        var textSection = memory.getBlock(".text");

        for (Pattern pattern : PATTERNS) {
            monitor.setMessage("Matching pattern: " + pattern.getLabel());

            // TODO: yield iterator?
            for (Address occurrence : pattern.findOccurrences(memory, textSection, monitor)) {
                documentOccurrence(occurrence, pattern);
            }

            monitor.setProgress(monitor.getProgress() + 1);
        }
    }

    private static final Pattern[] PATTERNS = new Pattern[] {
        // TODO: catalogue and add rest of the CFO patterns
        new JmpPattern(
            "RSP RET",
            // MOV RSP, <REG>
            "01001... 10001001 00...100 ..100100 " +
            // LEA <REG>, <TARGET>
            "01001... 10001101 00...101 ........ ........ ........ ........ " +
            // XCHG RSP, <REG>
            "01001... 10000111 00...100 ..100100 " +
            // RET
            "11000011"
        ),

        // Thanks wulf2k
        new SubstitutionPattern("Push rax(1)", "48 8d 64 24 f8 48 89 04 24"),
        new SubstitutionPattern("Push rax(2)", "48 89 44 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rbx(1)", "48 8d 64 24 f8 48 89 1c 24"),
        new SubstitutionPattern("Push rbx(2)", "48 89 5c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rcx(1)", "48 8d 64 24 f8 48 89 0c 24"),
        new SubstitutionPattern("Push rcx(2)", "48 89 4c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rdx(1)", "48 8d 64 24 f8 48 89 14 24"),
        new SubstitutionPattern("Push rdx(2)", "48 89 54 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rdi(1)", "48 8d 64 24 f8 48 89 3c 24"),
        new SubstitutionPattern("Push rdi(2)", "48 89 7c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rsi(1)", "48 8d 64 24 f8 48 89 34 24"),
        new SubstitutionPattern("Push rsi(2)", "48 89 74 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push rbp(1)", "48 8d 64 24 f8 48 89 2c 24"),
        new SubstitutionPattern("Push rbp(2)", "48 89 6c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r8(1)", "48 8d 64 24 f8 4c 89 04 24"),
        new SubstitutionPattern("Push r8(2)", "4c 89 44 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r9(1)", "48 8d 64 24 f8 4c 89 0c 24"),
        new SubstitutionPattern("Push r9(2)", "4c 89 4c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r10(1)", "48 8d 64 24 f8 4c 89 14 24"),
        new SubstitutionPattern("Push r10(2)", "4c 89 54 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r11(1)", "48 8d 64 24 f8 4c 89 1c 24"),
        new SubstitutionPattern("Push r11(2)", "4c 89 5c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r12(1)", "48 8d 64 24 f8 4c 89 24 24"),
        new SubstitutionPattern("Push r12(2)", "4c 89 64 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r13(1)", "48 8d 64 24 f8 4c 89 2c 24"),
        new SubstitutionPattern("Push r13(2)", "4c 89 6c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r14(1)", "48 8d 64 24 f8 4c 89 34 24"),
        new SubstitutionPattern("Push r14(2)", "4c 89 74 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Push r15(1)", "48 8d 64 24 f8 4c 89 3c 24"),
        new SubstitutionPattern("Push r15(2)", "4c 89 7c 24 f8 48 8d 64 24 f8"),
        new SubstitutionPattern("Pop rax(1)", "48 8d 64 24 08 48 8b 44 24 f8"),
        new SubstitutionPattern("Pop rax(2)", "48 8b 04 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rbx(1)", "48 8d 64 24 08 48 8b 5c 24 f8"),
        new SubstitutionPattern("Pop rbx(2)", "48 8b 1c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rcx(1)", "48 8d 64 24 08 48 8b 4c 24 f8"),
        new SubstitutionPattern("Pop rcx(2)", "48 8b 0c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rdx(1)", "48 8d 64 24 08 48 8b 54 24 f8"),
        new SubstitutionPattern("Pop rdx(2)", "48 8b 14 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rdi(1)", "48 8d 64 24 08 48 8b 7c 24 f8"),
        new SubstitutionPattern("Pop rdi(2)", "48 8b 3c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rsi(1)", "48 8d 64 24 08 48 8b 74 24 f8"),
        new SubstitutionPattern("Pop rsi(2)", "48 8b 34 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop rbp(1)", "48 8d 64 24 08 48 8b 6c 24 f8"),
        new SubstitutionPattern("Pop rbp(2)", "48 8b 2c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r8(1)", "48 8d 64 24 08 4c 8b 44 24 f8"),
        new SubstitutionPattern("Pop r8(2)", "4c 8b 04 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r9(1)", "48 8d 64 24 08 4c 8b 4c 24 f8"),
        new SubstitutionPattern("Pop r9(2)", "4c 8b 0c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r10(1)", "48 8d 64 24 08 4c 8b 54 24 f8"),
        new SubstitutionPattern("Pop r10(2)", "4c 8b 14 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r11(1)", "48 8d 64 24 08 4c 8b 5c 24 f8"),
        new SubstitutionPattern("Pop r11(2)", "4c 8b 1c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r12(1)", "48 8d 64 24 08 4c 8b 64 24 f8"),
        new SubstitutionPattern("Pop r12(2)", "4c 8b 24 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r13(1)", "48 8d 64 24 08 4c 8b 6c 24 f8"),
        new SubstitutionPattern("Pop r13(2)", "4c 8b 2c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r14(1)", "48 8d 64 24 08 4c 8b 74 24 f8"),
        new SubstitutionPattern("Pop r14(2)", "4c 8b 34 24 48 8d 64 24 08"),
        new SubstitutionPattern("Pop r15(1)", "48 8d 64 24 08 4c 8b 7c 24 f8"),
        new SubstitutionPattern("Pop r15(2)", "4c 8b 3c 24 48 8d 64 24 08"),
        new SubstitutionPattern("Stack Waste(1)", "48 8d 64 24 f8 48 8d 64 24 08"),
        new SubstitutionPattern("Stack Waste(2)", "48 8d 64 24 08 48 8d 64 24 f8"),
        new SubstitutionPattern("Ret", "48 8d 64 24 08 ff 64 24 f8"),
    };

    private interface Pattern {
        public String getLabel();
        public List<Address> findOccurrences(Memory memory, MemoryBlock textSection, TaskMonitor monitor);
    }

    private static class SubstitutionPattern implements Pattern {
        private String label;
        private String pattern;

        public SubstitutionPattern(String label, String pattern) {
            this.label = label;
            this.pattern = pattern;
        }

        public String getLabel() {
            return label;
        }

        public List<Address> findOccurrences(Memory memory, MemoryBlock textSection, TaskMonitor monitor){
            return PatternFinder.findAllBytePattern(
                memory,
                textSection.getStart(),
                textSection.getEnd(),
                this.pattern,
                monitor
            );
        }
    }

    private static class JmpPattern implements Pattern {
        private String label;
        private String pattern;

        public JmpPattern(String label, String pattern) {
            this.label = label;
            this.pattern = pattern;
        }

        public String getLabel() {
            return label;
        }

        public List<Address> findOccurrences(Memory memory, MemoryBlock textSection, TaskMonitor monitor){
            return PatternFinder.findAllBitPattern(
                memory,
                textSection.getStart(),
                textSection.getEnd(),
                this.pattern,
                monitor
            );
        }
    }

    private void documentOccurrence(Address occurrence, Pattern pattern) {
        var patternLabel = pattern.getLabel();

        // TODO: check if bookmark exists first
        //createBookmark(occurrence, "Obfuscation - Arxan", "Found pattern " + patternLabel);
        setPreComment(occurrence, "Found arxan pattern: " + patternLabel);

        println("Found occurrence of " + patternLabel + " at " + occurrence);
    }

    static class PatternFinder {
        public static List<Address> findAllBytePattern(
            Memory memory,
            Address start,
            Address end,
            String bytePattern,
            TaskMonitor monitor
        ) {
            var bytes = hexStringToByteArray(bytePattern);
            var mask = new byte[bytes.length];
            Arrays.fill(mask, (byte) 0xFF);

            var currentAddress = start;
            var result = new ArrayList<Address>();
            while (true) {
                var found = memory.findBytes(currentAddress, end, bytes, mask, true, monitor);
                if (found == null) {
                    break;
                }
                result.add(found);
                currentAddress = found.add(1);
            }
            return result;
        }

        // Thanks mrexodia for the pattern finding code
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

        // Thanks random chump on stackoverflow
        private static byte[] toByteArray(ArrayList<Byte> in) {
            final int n = in.size();
            byte ret[] = new byte[n];
            for (int i = 0; i < n; i++) {
                ret[i] = in.get(i);
            }
            return ret;
        }

        // Thanks random chump on stackoverflow
        public static byte[] hexStringToByteArray(String input) {
            var s = input.replaceAll(" ", "");

            int len = s.length();
            byte[] data = new byte[len / 2];
            for (int i = 0; i < len; i += 2) {
                data[i / 2] = (byte) ((Character.digit(s.charAt(i), 16) << 4)
                        + Character.digit(s.charAt(i+1), 16));
            }
            return data;
        }
    }
}
