/* ###
 * Memory-bounded, RESUMABLE Random-Forest function-start finder.
 *
 * Same idea as FindFunctionStartsRF.java, but instead of classifying the entire undefined
 * address space in one call (which builds one giant Map<Address,Double> and OOMs on a large
 * image like the 94MB deobf binary), it classifies the candidate addresses in fixed-size
 * CHUNKS, appending hits to a JSONL file and recording progress in a state file after each
 * chunk. Memory stays bounded to ~one chunk; a kill/crash resumes where it left off.
 *
 * Read-only: writes only to the external out/state files, never the program. (Training the
 * model fits in ~2G -- the OOM was always the monolithic classify, which this fixes.)
 *
 * Args: [0] threshold (0.80)  [1] maxStarts (500)  [2] minUndefinedRange (16)
 *       [3] chunkSize addresses (20000)  [4] outFile (JSONL)  [5] stateFile
 * Output JSONL lines: {"va":"0x...","score":0.93}. Resume: rerun with the same out/state files.
 *
 * @category Training
 */

import java.io.BufferedWriter;
import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.util.*;
import java.util.Map.Entry;

import ghidra.app.script.GhidraScript;
import ghidra.machinelearning.functionfinding.*;
import ghidra.program.model.address.*;

public class FindFunctionStartsRFChunked extends GhidraScript {

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		double threshold = args.length > 0 ? Double.parseDouble(args[0]) : 0.80d;
		int maxStarts = args.length > 1 ? Integer.parseInt(args[1]) : 500;
		long minRange = args.length > 2 ? Long.parseLong(args[2]) : 16L;
		int chunkSize = args.length > 3 ? Integer.parseInt(args[3]) : 20000;
		String outPath = args.length > 4 ? args[4] : "/tmp/rf-out.jsonl";
		String statePath = args.length > 5 ? args[5] : outPath + ".state";

		if (currentProgram == null) {
			println("RFCHUNK: no current program");
			return;
		}

		FunctionStartRFParams params = new FunctionStartRFParams(currentProgram);
		params.setMaxStarts(maxStarts);
		params.setMinFuncSize(16);
		params.setPreBytes(Arrays.asList(new Integer[] { 2, 8 }));
		params.setInitialBytes(Arrays.asList(new Integer[] { 8, 16 }));
		params.setFactors(Arrays.asList(new Integer[] { 10, 50 }));
		params.setIncludePrecedingAndFollowing(true);

		List<RandomForestRowObject> models = new ArrayList<>();
		new RandomForestTrainingTask(currentProgram, params, r -> models.add(r), 1000000L).run(monitor);
		if (models.isEmpty()) {
			println("RFCHUNK: no models trained");
			return;
		}
		Collections.sort(models,
			(x, y) -> Integer.compareUnsigned(x.getNumFalsePositives(), y.getNumFalsePositives()));
		RandomForestRowObject best = models.get(0);
		println("RFCHUNK: trained best precision=" + best.getPrecision() + " recall=" + best.getRecall());

		FunctionStartClassifier classifier = new FunctionStartClassifier(currentProgram, best,
			RandomForestFunctionFinderPlugin.FUNC_START);

		GetAddressesToClassifyTask getTask = new GetAddressesToClassifyTask(currentProgram, minRange);
		getTask.run(monitor);
		AddressSetView toClassify = getTask.getAddressesToClassify();
		println("RFCHUNK: candidate space = " + toClassify.getNumAddresses() + " addresses, chunk=" + chunkSize);

		// Resume: skip addresses at/below the last committed offset.
		long resumeAfter = readState(statePath);
		if (resumeAfter >= 0) {
			println("RFCHUNK: resuming after 0x" + Long.toHexString(resumeAfter));
		}

		long written = 0, done = 0;
		try (BufferedWriter out = new BufferedWriter(new FileWriter(outPath, true))) {
			AddressSet batch = new AddressSet();
			int n = 0;
			Address lastInBatch = null;
			AddressIterator it = toClassify.getAddresses(true);
			while (it.hasNext()) {
				if (monitor.isCancelled()) {
					break;
				}
				Address a = it.next();
				if (resumeAfter >= 0 && a.getOffset() <= resumeAfter) {
					continue;
				}
				batch.add(a);
				lastInBatch = a;
				if (++n >= chunkSize) {
					written += flush(classifier, batch, threshold, out);
					done += n;
					writeState(statePath, lastInBatch.getOffset());
					println("RFCHUNK: progress done=" + done + " written=" + written
						+ " at 0x" + Long.toHexString(lastInBatch.getOffset()));
					batch = new AddressSet();
					n = 0;
				}
			}
			if (n > 0 && !monitor.isCancelled()) {
				written += flush(classifier, batch, threshold, out);
				done += n;
				if (lastInBatch != null) {
					writeState(statePath, lastInBatch.getOffset());
				}
			}
		}
		println("RFCHUNK: DONE classified=" + done + " candidates_written=" + written + " -> " + outPath);
	}

	private long flush(FunctionStartClassifier classifier, AddressSet batch, double threshold,
			BufferedWriter out) throws Exception {
		Map<Address, Double> res = classifier.classify(batch, monitor);
		long w = 0;
		for (Entry<Address, Double> e : res.entrySet()) {
			if (e.getValue() >= threshold
					&& currentProgram.getFunctionManager().getFunctionAt(e.getKey()) == null) {
				out.write("{\"va\":\"0x" + Long.toHexString(e.getKey().getOffset())
					+ "\",\"score\":" + e.getValue() + "}\n");
				w++;
			}
		}
		out.flush();
		return w;
	}

	private long readState(String path) {
		try {
			File f = new File(path);
			if (!f.exists()) {
				return -1;
			}
			String s = new String(Files.readAllBytes(f.toPath())).trim();
			return s.isEmpty() ? -1 : Long.parseLong(s);
		}
		catch (Exception e) {
			return -1;
		}
	}

	private void writeState(String path, long offset) {
		try (BufferedWriter w = new BufferedWriter(new FileWriter(path, false))) {
			w.write(Long.toString(offset));
		}
		catch (IOException e) {
			println("RFCHUNK: WARN could not write state: " + e.getMessage());
		}
	}
}
