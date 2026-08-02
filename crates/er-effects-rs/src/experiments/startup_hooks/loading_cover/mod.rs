// Product loading-cover / title resource modules.
#![allow(unused_imports)]

use super::*;

pub(crate) mod scaleform_descriptor_guard {
    use super::*;

    include!("scaleform_descriptor_guard.rs");
}

pub(crate) use scaleform_descriptor_guard::*;

pub(crate) mod window_reconfig_observer {
    use super::*;

    include!("window_reconfig_observer.rs");
}

pub(crate) use window_reconfig_observer::*;

pub(crate) mod dlc_roots_self_heal {
    use super::*;

    include!("dlc_roots_self_heal.rs");
}

pub(crate) use dlc_roots_self_heal::*;
