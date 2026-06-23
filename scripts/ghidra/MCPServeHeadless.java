/* ###
 * Long-lived HEADLESS MCP server for a pre-warmed Ghidra program.
 *
 * Runs the 13bm GhidraMCP TCP server (ghidra.mcp.MCPServer) from a headless GhidraScript
 * instead of the GUI plugin, so a program stays LOADED AND WARM in one persistent
 * analyzeHeadless process and answers MCP tool calls instantly -- no GUI, no Xvfb, no
 * per-operation Ghidra startup. The 13bm Go bridge (launched by .mcp.json) connects to the
 * TCP port this script opens.
 *
 * Why this works headless: MCPServer/MCPContextProvider are plain objects, not GUI plugins.
 * The context provider only dereferences the PluginTool in two GUI-cursor tools
 * (get_current_address / get_current_function); passing a null tool disables just those two
 * (meaningless without a GUI cursor anyway) -- every Program-based tool (decompile, xrefs,
 * structs, list/search, basic blocks, ...) works off the Program object we set below.
 *
 * Run via analyzeHeadless ... -process [-readOnly] -postScript MCPServeHeadless.java PORT [STOPFILE].
 * The script blocks until STOPFILE appears or the monitor is cancelled, keeping the JVM (and
 * the loaded program) alive. See scripts/ghidra/mcp-ghidra-daemon.sh for lifecycle management.
 *
 * Args: [0] port (default 8765)   [1] stop-file path (optional; touch it to shut down cleanly)
 *
 * @category MCP
 */

import ghidra.app.script.GhidraScript;
import ghidra.mcp.MCPServer;

import java.io.File;

public class MCPServeHeadless extends GhidraScript {

	@Override
	protected void run() throws Exception {
		String[] args = getScriptArgs();
		int port = args.length > 0 ? Integer.parseInt(args[0]) : 8765;
		String stopPath = args.length > 1 ? args[1] : null;

		if (currentProgram == null) {
			println("MCP_HEADLESS: no current program -- run with -process against a saved program");
			return;
		}

		// null PluginTool: only the two GUI-cursor tools are disabled; all Program tools work.
		MCPServer server = new MCPServer(null);
		server.setPort(port);
		server.setRestrictToLocalhost(true);
		server.setCurrentProgram(currentProgram);
		server.startServer();

		if (!server.isRunning()) {
			println("MCP_HEADLESS: FAILED to start on port " + port + " (already in use?)");
			return;
		}
		println("MCP_HEADLESS: READY program='" + currentProgram.getName() + "' port=" + port);

		File stop = stopPath != null ? new File(stopPath) : null;
		try {
			while (!monitor.isCancelled()) {
				if (stop != null && stop.exists()) {
					println("MCP_HEADLESS: stop-file seen, shutting down");
					break;
				}
				Thread.sleep(1000);
			}
		}
		finally {
			server.stopServer();
			println("MCP_HEADLESS: stopped");
		}
	}
}
