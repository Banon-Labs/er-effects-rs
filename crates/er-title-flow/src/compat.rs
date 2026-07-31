// Compat seam: everything the moved cluster used to reach through the root
// crate's globs (`crate::*`, `crashlog::*`, `ffi::*`, `hooks::*`,
// `telemetry::*`, `experiments::*`). Populated during Stage B.
pub use crate::host::*;
pub use er_telemetry::counters::*;
