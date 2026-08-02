// Product (B) System>Quit modules that are being pulled out of the flat
// startup-hook include namespace one ownership slice at a time.
#![allow(unused_imports)]

pub(crate) mod row_identity {
    use super::super::*;

    include!("system_quit_row_identity.rs");
}

pub(crate) use row_identity::*;

pub(crate) mod save_dest_identity {
    use super::super::*;

    include!("save_dest_identity.rs");
}

pub(crate) use save_dest_identity::*;

pub(crate) mod save_picker_dim_overlay {
    use super::super::*;

    include!("save_picker_dim_overlay.rs");
}

pub(crate) use save_picker_dim_overlay::*;
