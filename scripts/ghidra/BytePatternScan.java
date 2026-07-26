// BytePatternScan.java
// Scans executable memory for instructions that set/clear ChrLoadState bit0 at [reg+0x1b4]:
//   OR  dword [reg+0x1b4], 1        -> 83 /1 ib : 83 (88..8F) B4 01 00 00 01
//   AND dword [reg+0x1b4], 0xfffffffe -> 81 /4 id: 81 (A0..A7) B4 01 00 00 FE FF FF FF
//   MOV dword [reg+0x1b4], imm32     -> C7 /0    : C7 (80..87) B4 01 00 00 ii ii ii ii
// Reports containing function for each hit. Uses masked findBytes (fast).
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;

public class BytePatternScan extends GhidraScript {
    void scan(String label, byte[] pat, byte[] mask) throws Exception {
        Memory mem = currentProgram.getMemory();
        FunctionManager fm = currentProgram.getFunctionManager();
        Address cur = currentProgram.getMinAddress();
        int found = 0;
        println("### " + label + " ###");
        while (cur != null) {
            Address a = mem.findBytes(cur, pat, mask, true, monitor);
            if (a == null) break;
            Function f = fm.getFunctionContaining(a);
            Instruction insn = getInstructionAt(a);
            println("  " + a + "  " + (f==null?"(no func)":f.getName(true)+" @ "+f.getEntryPoint())
                    + (insn==null?"":"  | "+insn.toString()));
            found++;
            cur = a.add(1);
            if (found >= 60) { println("  ...cap..."); break; }
        }
        println("  total=" + found);
    }
    public void run() throws Exception {
        // OR dword [reg+0x1b4], 1  (non-REX regs)
        scan("OR [reg+0x1b4],1",
            new byte[]{(byte)0x83,(byte)0x88,(byte)0xB4,0x01,0x00,0x00,0x01},
            new byte[]{(byte)0xFF,(byte)0xF8,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF});
        // REX.B OR dword [r8..r15+0x1b4],1 : 41 83 (88..8F) B4 01 00 00 01
        scan("OR [rNb+0x1b4],1 (REX.B)",
            new byte[]{(byte)0x41,(byte)0x83,(byte)0x88,(byte)0xB4,0x01,0x00,0x00,0x01},
            new byte[]{(byte)0xFF,(byte)0xFF,(byte)0xF8,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF});
        // AND dword [reg+0x1b4], 0xfffffffe : 81 (A0..A7) B4 01 00 00 FE FF FF FF
        scan("AND [reg+0x1b4],~1",
            new byte[]{(byte)0x81,(byte)0xA0,(byte)0xB4,0x01,0x00,0x00,(byte)0xFE,(byte)0xFF,(byte)0xFF,(byte)0xFF},
            new byte[]{(byte)0xFF,(byte)0xF8,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF});
        // MOV dword [reg+0x1b4], imm32 : C7 (80..87) B4 01 00 00 ?? ?? ?? ??
        scan("MOV [reg+0x1b4],imm32",
            new byte[]{(byte)0xC7,(byte)0x80,(byte)0xB4,0x01,0x00,0x00,0x00,0x00,0x00,0x00},
            new byte[]{(byte)0xFF,(byte)0xF8,(byte)0xFF,(byte)0xFF,(byte)0xFF,(byte)0xFF,0x00,0x00,0x00,0x00});
    }
}
