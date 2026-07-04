use std::{fs, path::PathBuf};

use er_safe_input::{SafeButton, SafeInputAction, SafeInputConfig, SafeInputError};

pub mod bnd4;
pub mod stats;

include!("lib_parts/chunk_01.rs");
include!("lib_parts/chunk_02.rs");
