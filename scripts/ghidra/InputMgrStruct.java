// InputMgrStruct.java -- dump CSMenuManImp + MenuWindow struct layouts (fields near the input region)
import ghidra.app.script.GhidraScript;
import ghidra.program.model.data.*;

public class InputMgrStruct extends GhidraScript {
    void dump(String type, long lo, long hi) {
        DataTypeManager dtm = currentProgram.getDataTypeManager();
        java.util.Iterator<DataType> it = dtm.getAllDataTypes();
        int shown=0;
        while (it.hasNext()) {
            DataType dt = it.next();
            if (!(dt instanceof Structure)) continue;
            if (!dt.getName().equalsIgnoreCase(type)) continue;
            Structure s = (Structure) dt;
            println("=== " + s.getName() + " size=0x" + Long.toHexString(s.getLength()) + " ===");
            for (DataTypeComponent c : s.getDefinedComponents()) {
                long off = c.getOffset();
                if (off < lo || off > hi) continue;
                String nm = c.getFieldName();
                println("  +0x" + Long.toHexString(off) + "  " + (nm==null?"(unnamed)":nm)
                        + " : " + c.getDataType().getName() + "  (len=0x" + Long.toHexString(c.getLength()) + ")");
            }
            shown++;
        }
        if (shown==0) println("  no struct named " + type);
    }
    public void run() throws Exception {
        println("### CSMenuManImp fields +0x60..+0x200");
        dump("CSMenuManImp", 0x60, 0x200);
        println("### MenuWindow fields +0x170..+0x190 (eventId field246_0x180)");
        dump("MenuWindow", 0x170, 0x190);
    }
}
