import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;

// Usage: RtVtbl <mode> <va> <count>
//   mode=disasm : disassemble <count> instructions starting exactly at <va>
//   mode=ptrs   : read <count> pointer-sized words at <va>, resolve each to symbol/function
//   mode=refs   : list references TO <va>
public class RtVtbl extends GhidraScript {
    @Override public void run() throws Exception {
        String[] args = getScriptArgs();
        String mode = args[0];
        long va = Long.decode(args[1]);
        int count = args.length > 2 ? Integer.decode(args[2]) : 16;
        Listing lst = currentProgram.getListing();
        Memory mem = currentProgram.getMemory();
        SymbolTable st = currentProgram.getSymbolTable();
        if (mode.equals("disasm")) {
            Address a = toAddr(va);
            for (int i = 0; i < count; i++) {
                Instruction insn = lst.getInstructionAt(a);
                if (insn == null) { println("  (no insn at " + a + ")"); break; }
                println("  " + insn.getAddress() + "  " + insn);
                a = insn.getAddress().add(insn.getLength());
            }
        } else if (mode.equals("ptrs")) {
            for (int i = 0; i < count; i++) {
                Address slot = toAddr(va + (long) i * 8);
                long p = mem.getLong(slot) & 0xffffffffffffffffL;
                Address tgt = toAddr(p);
                Function f = getFunctionContaining(tgt);
                Symbol s = st.getPrimarySymbol(tgt);
                println("  [" + slot + "] = " + Long.toHexString(p)
                    + "  fn=" + (f != null ? f.getName() + "@" + f.getEntryPoint() : "-")
                    + "  sym=" + (s != null ? s.getName() : "-"));
            }
        } else if (mode.equals("w4")) {
            for (int i = 0; i < count; i++) {
                Address slot = toAddr(va + (long) i * 4);
                long p = mem.getInt(slot) & 0xffffffffL;
                // try as image-relative (base 0x140000000) and as raw
                Address tgt = toAddr(0x140000000L + p);
                Function f = getFunctionContaining(tgt);
                Symbol s = st.getPrimarySymbol(tgt);
                Address tgt2 = toAddr(p);
                Function f2 = getFunctionContaining(tgt2);
                println("  [" + slot + "] raw=" + Long.toHexString(p)
                    + "  +base=" + tgt + " fn=" + (f != null ? f.getName() : "-") + (s != null ? "/" + s.getName() : "")
                    + "  asIs=" + (f2 != null ? f2.getName() : "-"));
            }
        } else if (mode.equals("str")) {
            StringBuilder sb = new StringBuilder();
            Address a = toAddr(va);
            for (int i = 0; i < count && i < 512; i++) {
                byte b = mem.getByte(a.add(i));
                if (b == 0) break;
                sb.append((char) (b & 0xff));
            }
            println("  str@" + toAddr(va) + " = \"" + sb + "\"");
        } else if (mode.equals("refs")) {
            ReferenceIterator it = currentProgram.getReferenceManager().getReferencesTo(toAddr(va));
            while (it.hasNext()) {
                Reference r = it.next();
                Function f = getFunctionContaining(r.getFromAddress());
                println("  from " + r.getFromAddress() + "  " + r.getReferenceType()
                    + "  in=" + (f != null ? f.getName() + "@" + f.getEntryPoint() : "-"));
            }
        }
    }
}
