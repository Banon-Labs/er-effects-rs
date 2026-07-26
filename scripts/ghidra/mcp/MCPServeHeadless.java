/* ###
 * Long-lived HEADLESS MCP server for a pre-warmed Ghidra program, with PERSISTENT edits.
 *
 * Runs the 13bm GhidraMCP TCP server (ghidra.mcp.MCPServer) from a headless GhidraScript so a
 * program stays LOADED AND WARM in one analyzeHeadless process and answers MCP tool calls
 * instantly -- no GUI, no Xvfb, no per-operation Ghidra startup. The Go bridge (.mcp.json)
 * connects to the TCP port this opens.
 *
 * Why MCPServer runs headless: it and MCPContextProvider are plain objects, not GUI plugins; the
 * context provider only dereferences the PluginTool in two GUI-cursor tools
 * (get_current_address / get_current_function), so a null tool disables just those two.
 *
 * PERSISTENCE (the subtle part): GhidraScript wraps run() in an auto-transaction named after this
 * class. While it is open, MCP mutations cannot commit (isChanged stays false) and the program
 * cannot be saved ("Unable to lock due to active transaction"); because the daemon blocks forever,
 * that transaction would never close, so edits would never persist. We call end(true) right after
 * the server starts to commit+close it -- then MCP mutations (their own short transactions) commit
 * normally and periodic save() persists them to the project. end() is idempotent, so the
 * framework's own end(true) after run() returns is a safe no-op.
 *
 * Args: [0] port (default 8765)   [1] stop-file path (touch to shut down cleanly)
 *       [2] auto-save interval seconds (default 60, 0 = off)
 *
 * @category MCP
 */

import ghidra.app.script.GhidraScript;
import ghidra.framework.model.TransactionInfo;
import ghidra.mcp.MCPServer;

import java.io.File;

public class MCPServeHeadless extends GhidraScript {

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		int port = args.length > 0 ? Integer.parseInt(args[0]) : 8765;
		String stopPath = args.length > 1 ? args[1] : null;
		int saveEverySec = args.length > 2 ? Integer.parseInt(args[2]) : 60;

		if (currentProgram == null) {
			println("MCP_HEADLESS: no current program -- run with -process against a saved program");
			return;
		}

		MCPServer server = new MCPServer(null);
		server.setPort(port);
		server.setRestrictToLocalhost(true);
		server.setCurrentProgram(currentProgram);
		server.startServer();
		if (!server.isRunning()) {
			println("MCP_HEADLESS: FAILED to start on port " + port + " (already in use?)");
			return;
		}

		// Close GhidraScript's auto-transaction so MCP edits commit and save() can persist.
		end(true);

		println("MCP_HEADLESS: READY program='" + currentProgram.getName() + "' port=" + port
			+ (saveEverySec > 0 ? " autosave=" + saveEverySec + "s" : ""));

		File stop = stopPath != null ? new File(stopPath) : null;
		int sinceSave = 0;
		try {
			while (!monitor.isCancelled()) {
				if (stop != null && stop.exists()) {
					println("MCP_HEADLESS: stop-file seen, shutting down");
					break;
				}
				Thread.sleep(1000);
				if (saveEverySec > 0 && ++sinceSave >= saveEverySec) {
					sinceSave = 0;
					autoSave();
				}
			}
		}
		finally {
			autoSave(); // final flush of any pending edits on clean shutdown
			server.stopServer();
			println("MCP_HEADLESS: stopped");
		}
	}

	// Persist committed edits to the project. Skips when nothing changed, when an MCP mutation
	// transaction is in flight (save needs the lock -- retried next interval), or for read-only
	// programs; all failure paths are caught so the daemon keeps serving.
	private void autoSave() {
		try {
			if (currentProgram == null || !currentProgram.isChanged()) {
				return;
			}
			TransactionInfo ti = currentProgram.getCurrentTransactionInfo();
			if (ti != null) {
				return; // a mutation is mid-flight; save on the next cycle
			}
			currentProgram.save("MCP headless auto-save", monitor);
			println("MCP_HEADLESS: saved");
		}
		catch (Exception e) {
			println("MCP_HEADLESS: save skipped: " + e.getMessage());
		}
	}
}
