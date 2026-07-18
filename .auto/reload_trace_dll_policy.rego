package auto.reload_trace_dll

import rego.v1

default allow := false

# This policy describes the manual diagnostic DLL requested for same-character reload
# RE work. The DLL may install native trampolines and write a log file only. It must
# not add runtime env gates, drive input, redirect or swap saves, call product autoload
# code, or directly write game memory. The companion checker builds the input facts by
# scanning crates/er-reload-trace-dll.

minimum_hook_count := 32

allow if {
	input.crate_path == "crates/er-reload-trace-dll"
	input.cdylib == true
	input.has_dllmain == true
	input.has_minhook == true
	input.calls_original_trampolines == true
	input.hook_count >= minimum_hook_count
	count(deny) == 0
}

deny contains message if {
	input.env_gate_count > 0
	message := "reload trace DLL must not contain std::env::var or ER_EFFECTS_* runtime env gates"
}

deny contains message if {
	input.input_api_count > 0
	message := "reload trace DLL must not use input/window-control APIs; manual user input remains native/vanilla"
}

deny contains message if {
	input.save_or_loader_count > 0
	message := "reload trace DLL must not import save-loader/autoload/save-redirect/product load machinery"
}

deny contains message if {
	input.direct_game_write_count > 0
	message := "reload trace DLL must not directly write game memory; MinHook trampoline installation plus log-file writes are the only mutations allowed"
}

deny contains message if {
	input.cdylib != true
	message := "reload trace DLL crate must build as a cdylib"
}

deny contains message if {
	input.has_dllmain != true
	message := "reload trace DLL must expose DllMain for native loader attach"
}

deny contains message if {
	input.has_minhook != true
	message := "reload trace DLL must use MinHook trampolines for pass-through instrumentation"
}

deny contains message if {
	input.calls_original_trampolines != true
	message := "reload trace hooks must call original trampolines and return their result"
}

deny contains message if {
	input.hook_count < minimum_hook_count
	message := sprintf("reload trace DLL must install at least %d native trace hooks", [minimum_hook_count])
}
