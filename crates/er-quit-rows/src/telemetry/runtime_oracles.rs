// Runtime telemetry and in-process oracle writers.
//
// Split into focused include files to keep the hard file-size gate useful while
// preserving the original flat `telemetry` module namespace.

include!("runtime_oracles/bootstrap.rs");
include!("runtime_oracles/write_telemetry.rs");
include!("runtime_oracles/game_man_snapshot.rs");
include!("runtime_oracles/write_oracle.rs");
include!("runtime_oracles/portrait_framing_oracles.rs");
include!("runtime_oracles/portrait_bridge_hold_oracles.rs");
include!("runtime_oracles/write_title_load_route_oracles.rs");
include!("runtime_oracles/oracles_engine_loop.rs");
include!("runtime_oracles/oracles_character_identity.rs");
include!("runtime_oracles/oracles_render_state.rs");
include!("runtime_oracles/oracles_title_visuals.rs");
include!("runtime_oracles/oracles_loading_cover.rs");
include!("runtime_oracles/oracles_portrait_pipeline.rs");
include!("runtime_oracles/oracles_loading_screen_live.rs");
include!("runtime_oracles/write_game_module_oracles.rs");
#[cfg(test)]
include!("runtime_oracles/write_game_module_oracles_tests.rs");
