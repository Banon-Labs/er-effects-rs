//! In-process D3D12 readback of the live profile-portrait offscreen render target.
//!
//! P1 already drives the live character model to render into a `CSGxTexture`'s GPU child (an
//! `ID3D12Resource`, possibly behind a `CSOffscreenGxTexture` wrapper). This module copies that
//! render target's subresource 0 into a CPU-visible READBACK buffer and de-swizzles it into a
//! tightly-packed `width*height*4` RGBA8 buffer, which the now-loading forge then feeds to the game's
//! in-memory TPF factory -- so the loading screen shows the REAL rendered head instead of the
//! magenta/yellow checker placeholder.
//!
//! Safety contract (see TASK):
//! * The game's `ID3D12Resource` is wrapped WITHOUT taking ownership (`from_raw_borrowed`), so we
//!   NEVER Release it.
//! * We create our OWN command queue/allocator/list/fence; we NEVER touch the game's queue.
//! * Every fallible COM call is `?`/`ok()?`-checked and the whole body is `catch_unwind`-wrapped:
//!   this runs on the game thread and must never panic or crash; on any failure it returns `None`.

#![allow(unused_imports)]

include!("gpu_readback/resource_readback.rs");
include!("gpu_readback/cached_depth_readback.rs");
include!("gpu_readback/depth_mask_upload.rs");
include!("gpu_readback/overlay_composite.rs");
