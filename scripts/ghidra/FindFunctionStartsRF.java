/* ###
 * Read-only Random-Forest function-start finder for the ER reverse-engineering workflow.
 *
 * Derived from Ghidra's MachineLearning extension example
 * (Ghidra/Extensions/MachineLearning/ghidra_scripts/FindFunctionsRFExampleScript.java),
 * but with every MUTATING step removed: it trains the random forest, classifies the
 * undefined byte ranges, and EMITS the candidate function-start VAs as JSON. It never
 * calls DisassembleCommand/CreateFunctionCmd, so it is safe to run under
 * `analyzeHeadless ... -process -readOnly` against a saved, already-analyzed program.
 *
 * The trained model learns from the program's ALREADY-KNOWN functions, so the target
 * program must have a good set of defined functions (auto-analysis or imported symbols).
 *
 * Output: a single JSON object between RF_RESULT_JSON_BEGIN / RF_RESULT_JSON_END markers
 * so the caller can slice it out of the headless log deterministically.
 *
 * Args (all optional, positional):
 *   [0] score threshold   (double, default 0.80) -- minimum classifier confidence to report
 *   [1] maxStarts         (int,    default 1000) -- training-set cap (FunctionStartRFParams)
 *   [2] minUndefinedRange (long,   default 16)   -- min undefined run length to classify
 *
 * @category Training
 */

import java.util.*;
import java.util.Map.Entry;
import java.util.stream.Collectors;

import ghidra.app.script.GhidraScript;
import ghidra.machinelearning.functionfinding.*;
import ghidra.program.model.address.*;

public class FindFunctionStartsRF extends GhidraScript {

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		double threshold = args.length > 0 ? Double.parseDouble(args[0]) : 0.80d;
		int maxStarts = args.length > 1 ? Integer.parseInt(args[1]) : 1000;
		long minUndefinedRange = args.length > 2 ? Long.parseLong(args[2]) : 16L;

		// Match the extension example's feature-extraction sweep; the trainer evaluates the
		// cross product of (preBytes x initialBytes x factors) and we keep the best model.
		FunctionStartRFParams params = new FunctionStartRFParams(currentProgram);
		params.setMaxStarts(maxStarts);
		params.setMinFuncSize(16);
		params.setPreBytes(Arrays.asList(new Integer[] { 2, 8 }));
		params.setInitialBytes(Arrays.asList(new Integer[] { 8, 16 }));
		params.setFactors(Arrays.asList(new Integer[] { 10, 50 }));
		params.setIncludePrecedingAndFollowing(true);

		long testSetMax = 1000000L;
		List<RandomForestRowObject> trainedModels = new ArrayList<>();
		RandomForestTrainingTask trainingTask =
			new RandomForestTrainingTask(currentProgram, params, r -> trainedModels.add(r), testSetMax);
		trainingTask.run(monitor);

		if (trainedModels.isEmpty()) {
			println("RF_RESULT_JSON_BEGIN");
			println("{\"error\":\"no models trained -- program likely has too few defined functions\"}");
			println("RF_RESULT_JSON_END");
			return;
		}

		// Lowest false-positive count == best model, same ranking the example uses.
		Collections.sort(trainedModels,
			(x, y) -> Integer.compareUnsigned(x.getNumFalsePositives(), y.getNumFalsePositives()));
		RandomForestRowObject best = trainedModels.get(0);

		FunctionStartClassifier classifier = new FunctionStartClassifier(currentProgram, best,
			RandomForestFunctionFinderPlugin.FUNC_START);

		GetAddressesToClassifyTask getAddressTask =
			new GetAddressesToClassifyTask(currentProgram, minUndefinedRange);
		getAddressTask.run(monitor);
		AddressSetView toClassify = getAddressTask.getAddressesToClassify();

		Map<Address, Double> potentialStarts = classifier.classify(toClassify, monitor);

		// Keep only confident hits that are not already defined as functions, high score first.
		List<Entry<Address, Double>> hits = potentialStarts.entrySet().stream()
			.filter(e -> e.getValue() >= threshold)
			.filter(e -> currentProgram.getFunctionManager().getFunctionAt(e.getKey()) == null)
			.sorted((a, b) -> Double.compare(b.getValue(), a.getValue()))
			.collect(Collectors.toList());

		long imageBase = currentProgram.getImageBase().getOffset();

		StringBuilder sb = new StringBuilder();
		sb.append("{");
		sb.append("\"program\":\"").append(jsonEscape(currentProgram.getName())).append("\",");
		sb.append("\"image_base\":\"0x").append(Long.toHexString(imageBase)).append("\",");
		sb.append("\"threshold\":").append(threshold).append(",");
		sb.append("\"best_model\":{")
			.append("\"pre_bytes\":").append(best.getNumPreBytes()).append(",")
			.append("\"initial_bytes\":").append(best.getNumInitialBytes()).append(",")
			.append("\"sampling_factor\":").append(best.getSamplingFactor()).append(",")
			.append("\"false_positives\":").append(best.getNumFalsePositives()).append(",")
			.append("\"precision\":").append(best.getPrecision()).append(",")
			.append("\"recall\":").append(best.getRecall())
			.append("},");
		sb.append("\"count\":").append(hits.size()).append(",");
		sb.append("\"candidates\":[");
		for (int i = 0; i < hits.size(); i++) {
			Entry<Address, Double> e = hits.get(i);
			if (i > 0) {
				sb.append(",");
			}
			sb.append("{\"va\":\"0x").append(Long.toHexString(e.getKey().getOffset()))
				.append("\",\"score\":").append(e.getValue()).append("}");
		}
		sb.append("]}");

		println("RF_RESULT_JSON_BEGIN");
		println(sb.toString());
		println("RF_RESULT_JSON_END");
	}

	private static String jsonEscape(String s) {
		return s.replace("\\", "\\\\").replace("\"", "\\\"");
	}
}
