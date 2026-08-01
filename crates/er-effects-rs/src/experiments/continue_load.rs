//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use super::*;

mod product_continue;
pub(crate) use product_continue::*;

mod slot_resolution;
pub(crate) use slot_resolution::*;
