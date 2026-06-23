/* ###
 * Run Ghidra auto-analysis with LIVE PROGRESS logging (closes the headless "no progress %" gap).
 *
 * analyzeHeadless tracks analysis progress internally via the script's TaskMonitor but never
 * surfaces it. This postScript polls that monitor from a daemon thread and prints a heartbeat
 * line every few seconds: the current analyzer message plus its progress, so a headless import
 * is observable instead of silent. Run it as the postScript of a `-import ... -noanalysis` run
 * (see scripts/ghidra/import-deobf.sh); analyzeHeadless saves the analyzed program afterward.
 *
 * Progress note: Ghidra runs many analyzers in sequence and each sets its OWN monitor
 * max/progress, so the percentage is per-CURRENT-analyzer, not a global ETA. The message names
 * the running analyzer, which is the useful signal ("which phase, is it advancing").
 *
 * Args: [0] poll interval seconds (default 3)
 *
 * @category Analysis
 */

import ghidra.app.script.GhidraScript;
import ghidra.util.task.TaskMonitor;

public class AnalyzeWithProgress extends GhidraScript {

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		long intervalMs = (args.length > 0 ? Long.parseLong(args[0]) : 3L) * 1000L;

		if (currentProgram == null) {
			println("ANALYZE_PROGRESS: no current program");
			return;
		}

		final TaskMonitor mon = monitor;
		final long startNanos = System.nanoTime();
		final boolean[] done = { false };

		Thread poller = new Thread(() -> {
			String last = "";
			while (!done[0]) {
				try {
					long prog = mon.getProgress();
					long max = mon.getMaximum();
					String msg = mon.getMessage();
					long elapsed = (System.nanoTime() - startNanos) / 1_000_000_000L;
					String pct = (max > 0 && prog >= 0) ? String.format("%d%%", (100 * prog) / max) : "--";
					String line = "ANALYZE_PROGRESS: t+" + elapsed + "s [" + pct + "] "
						+ (msg != null ? msg : "(working)") + " (" + prog + "/" + max + ")";
					// Skip identical consecutive lines to keep the log readable.
					if (!line.equals(last)) {
						println(line);
						last = line;
					}
					Thread.sleep(intervalMs);
				}
				catch (InterruptedException e) {
					return;
				}
				catch (Exception e) {
					// monitor read races are harmless; keep polling.
				}
			}
		}, "analyze-progress-poller");
		poller.setDaemon(true);

		println("ANALYZE_PROGRESS: starting auto-analysis of '" + currentProgram.getName() + "'");
		poller.start();
		try {
			analyzeAll(currentProgram);
		}
		finally {
			done[0] = true;
			poller.interrupt();
		}
		long total = (System.nanoTime() - startNanos) / 1_000_000_000L;
		println("ANALYZE_PROGRESS: DONE in " + total + "s, functions=" +
			currentProgram.getFunctionManager().getFunctionCount());
	}
}
