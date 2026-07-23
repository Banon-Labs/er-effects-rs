use std::{fs, path::PathBuf};

use er_safe_input::{SafeButton, SafeInputAction, SafeInputConfig, SafeInputError};

pub mod bnd4;
pub mod stats;

include!("lib_parts/context.rs");
include!("lib_parts/load_methods.rs");
