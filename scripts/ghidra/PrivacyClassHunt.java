import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import java.util.*;

// Tight identity hunt for the boot legal/agreement dialog.
// (A) symbol names containing exact-cased class keywords.
// (B) scan defined-string / memory for MSVC RTTI type-descriptor names
//     (".?AV...@@") and any ASCII string containing the keywords, printing the VA.
public class PrivacyClassHunt extends GhidraScript {
    public void run() throws Exception {
        SymbolTable st = currentProgram.getSymbolTable();
        Listing listing = currentProgram.getListing();
        Memory mem = currentProgram.getMemory();
        println("[PCH] imagebase=0x"+Long.toHexString(currentProgram.getImageBase().getOffset()));

        // exact-cased keywords (avoid substring collisions like ToState)
        String[] kw = {"Tos","Eula","Privacy","Agreement","MultiLang","License",
                       "ToSMenu","TosMenu","AgreementMenu","Legal","TermsOf"};

        println("[PCH] ===== (A) symbol-name hits =====");
        int a=0;
        SymbolIterator sit = st.getAllSymbols(true);
        while (sit.hasNext()) {
            Symbol s = sit.next();
            String n = s.getName();
            for (String k : kw) {
                if (n.contains(k)) {
                    println(String.format("[PCH-SYM] %s @ %s ns=%s type=%s", n, s.getAddress(),
                        s.getParentNamespace()!=null?s.getParentNamespace().getName(true):"-", s.getSymbolType()));
                    a++;
                    break;
                }
            }
            if (a>250) { println("[PCH] (sym cap)"); break; }
        }
        println("[PCH] symA="+a);

        // (B) defined strings containing keywords (RTTI names live in .data/.rdata as defined strings)
        println("[PCH] ===== (B) defined-string hits =====");
        int b=0;
        DataIterator di = listing.getDefinedData(true);
        while (di.hasNext()) {
            Data d = di.next();
            String t = d.getDataType().getName().toLowerCase();
            if (!(t.contains("string") || t.contains("char"))) continue;
            Object v = d.getValue();
            if (v == null) continue;
            String sv = v.toString();
            for (String k : kw) {
                if (sv.contains(k)) {
                    println(String.format("[PCH-STR] %s : \"%s\"", d.getAddress(), sv.length()>120?sv.substring(0,120):sv));
                    b++;
                    break;
                }
            }
            if (b>300) { println("[PCH] (str cap)"); break; }
        }
        println("[PCH] strB="+b);

        // (C) raw byte scan of memory blocks for the literal ASCII substrings (catches
        //     undefined RTTI descriptors). Bounded to initialized blocks.
        println("[PCH] ===== (C) raw-byte ASCII scan =====");
        byte[][] needles = new byte[][]{
            "Privacy".getBytes(), "Agreement".getBytes(), "MultiLang".getBytes(),
            ".?AVCS".getBytes(), "TosMenu".getBytes(), "ToSMenu".getBytes(),
            "EulaMenu".getBytes()
        };
        int c=0;
        for (MemoryBlock blk : mem.getBlocks()) {
            if (!blk.isInitialized()) continue;
            if (blk.getName().startsWith(".text")) continue; // data only
            long sz = blk.getSize();
            if (sz > 0x6000000) continue;
            byte[] buf = new byte[(int)sz];
            try { blk.getBytes(blk.getStart(), buf); } catch (Exception e) { continue; }
            for (byte[] nd : needles) {
                int from=0;
                while (true) {
                    int idx = indexOf(buf, nd, from);
                    if (idx<0) break;
                    from = idx+1;
                    // build a short readable window
                    int end=idx; while (end<buf.length && end<idx+64 && buf[end]>=0x20 && buf[end]<0x7f) end++;
                    String w = new String(buf, idx, end-idx);
                    Address at = blk.getStart().add(idx);
                    // only report RTTI-ish or keyword strings (skip short noise)
                    if (w.length()>=6) {
                        println(String.format("[PCH-RAW] %s (%s): %s", at, blk.getName(), w));
                        c++;
                    }
                    if (c>400) break;
                }
                if (c>400) break;
            }
            if (c>400) { println("[PCH] (raw cap)"); break; }
        }
        println("[PCH] rawC="+c);
        println("[PCH] DONE");
    }

    int indexOf(byte[] hay, byte[] nee, int from) {
        outer:
        for (int i=from; i<=hay.length-nee.length; i++) {
            for (int j=0;j<nee.length;j++) if (hay[i+j]!=nee[j]) continue outer;
            return i;
        }
        return -1;
    }
}
