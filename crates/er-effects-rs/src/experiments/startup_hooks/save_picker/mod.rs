// Product (A) boot missing-save picker modules.
#![allow(unused_imports)]

use super::*;

pub(crate) mod os_dialog {
    use super::*;

    include!("save_picker_os_dialog.rs");
}

pub(crate) use os_dialog::*;

pub(crate) mod boot {
    use super::*;

    include!("save_picker_boot.rs");
}

pub(crate) use boot::*;

pub(crate) mod surface {
    use super::*;

    include!("save_picker_surface.rs");
}

pub(crate) use surface::*;
