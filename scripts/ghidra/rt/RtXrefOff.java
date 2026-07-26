// Find instructions that reference a given constant offset (as +0xNNN displacement) - heuristic scan
// Usage: RtXrefOff.java 0xb3140
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.scalar.Scalar;
import java.util.*;

public class RtXrefOff extends GhidraScript {
  public void run() throws Exception {
    String[] a = getScriptArgs();
    long off = Long.decode(a[0]);
    Listing lst = currentProgram.getListing();
    InstructionIterator it = lst.getInstructions(true);
    Set<Function> hits = new LinkedHashSet<>();
    while (it.hasNext()) {
      Instruction ins = it.next();
      int n = ins.getNumOperands();
      boolean matched = false;
      for (int i=0;i<n && !matched;i++){
        for (Object o : ins.getOpObjects(i)) {
          if (o instanceof Scalar) {
            long v = ((Scalar)o).getValue();
            if (v == off) { matched = true; break; }
          }
        }
      }
      if (matched) {
        Function f = lst.getFunctionContaining(ins.getAddress());
        String fn = f==null? "??" : f.getName();
        println(ins.getAddress()+"  "+fn+"  "+ins);
        if (f!=null) hits.add(f);
      }
    }
    println("==== unique functions: "+hits.size());
  }
}
