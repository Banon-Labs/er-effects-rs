// Runtime telemetry and in-process oracle writers.
//
// Split into focused include files to keep the hard file-size gate useful while
// preserving the original flat `telemetry` module namespace.

include!("runtime_oracles/bootstrap.rs");
include!("runtime_oracles/write_telemetry.rs");
include!("runtime_oracles/game_man_snapshot.rs");
include!("runtime_oracles/write_oracle.rs");
include!("runtime_oracles/write_game_module_oracles.rs");
