import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import java.io.*;
import java.util.*;

// Emit one TSV line per function: entryVA<TAB>name<TAB>hex(first N bytes)
// Only functions whose entry lies in an executable initialized memory block.
// Output path is scriptArg[0]; N is scriptArg[1] (default 32).
public class DumpAllFuncBytes extends GhidraScript {
    public void run() throws Exception {
        String[] args = getScriptArgs();
        String out = (args.length >= 1) ? args[0] : "/home/banon/projects/er-effects-rs/scratchpad/dump_funcs.tsv";
        int n = (args.length >= 2) ? Integer.parseInt(args[1]) : 32;

        FunctionManager fm = currentProgram.getFunctionManager();
        Memory mem = currentProgram.getMemory();
        BufferedWriter w = new BufferedWriter(new FileWriter(out));
        long count = 0, skipped = 0;
        FunctionIterator it = fm.getFunctions(true);
        byte[] b = new byte[n];
        while (it.hasNext()) {
            Function f = it.next();
            Address ep = f.getEntryPoint();
            MemoryBlock blk = mem.getBlock(ep);
            if (blk == null || !blk.isExecute() || !blk.isInitialized()) { skipped++; continue; }
            try {
                mem.getBytes(ep, b);
            } catch (Exception e) { skipped++; continue; }
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < n; i++) sb.append(String.format("%02x", b[i] & 0xff));
            String name = f.getName().replace('\t', ' ').replace('\n', ' ');
            w.write(Long.toHexString(ep.getOffset()));
            w.write('\t');
            w.write(name);
            w.write('\t');
            w.write(sb.toString());
            w.write('\n');
            count++;
        }
        w.close();
        println("[DAFB] wrote " + count + " functions, skipped " + skipped + " -> " + out);
        println("[DAFB] DONE");
    }
}
