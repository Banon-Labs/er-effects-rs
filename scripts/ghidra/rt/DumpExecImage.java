import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import java.io.*;

// Export every executable+initialized memory block of the runtime DUMP to a flat
// RVA-aligned raw image (file offset == VA - imageBase), matching the layout of
// eldenring-deobf.bin. This lets a local tool byte-match ANY dump VA against the
// deobf image to ground-truth the dump->deobf shift, with no Ghidra per query.
// scriptArg[0] = output path; scriptArg[1] = image base hex (default 0x140000000).
public class DumpExecImage extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        String out = (args.length >= 1) ? args[0]
            : "/home/banon/projects/er-effects-rs/scratchpad/dump-exec.bin";
        long base = (args.length >= 2) ? Long.decode(args[1]) : 0x140000000L;

        Memory mem = currentProgram.getMemory();
        // size the file to cover the highest exec block end.
        long maxEnd = 0;
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isExecute() || !b.isInitialized()) continue;
            long end = b.getEnd().getOffset() - base + 1;
            if (end > maxEnd) maxEnd = end;
        }
        println("[DEI] image size = 0x" + Long.toHexString(maxEnd));
        RandomAccessFile raf = new RandomAccessFile(out, "rw");
        raf.setLength(maxEnd);
        byte[] buf = new byte[1 << 20];
        long written = 0;
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isExecute() || !b.isInitialized()) continue;
            long start = b.getStart().getOffset();
            long size = b.getSize();
            long off = start - base;
            println("[DEI] block " + b.getName() + " va=0x" + Long.toHexString(start)
                + " size=0x" + Long.toHexString(size) + " -> off=0x" + Long.toHexString(off));
            Address a = b.getStart();
            long remaining = size;
            raf.seek(off);
            while (remaining > 0) {
                int chunk = (int) Math.min(buf.length, remaining);
                mem.getBytes(a, buf, 0, chunk);
                raf.write(buf, 0, chunk);
                a = a.add(chunk);
                remaining -= chunk;
                written += chunk;
            }
        }
        raf.close();
        println("[DEI] wrote 0x" + Long.toHexString(written) + " bytes -> " + out);
        println("[DEI] DONE");
    }
}
