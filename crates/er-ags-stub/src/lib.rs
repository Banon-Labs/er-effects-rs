//! Stub `amd_ags_x64.dll` for RenderDoc capture runs (bd RENDERDOC-ags-fix-is-STUB-amd-ags-dll).
//!
//! ER links AMD's AGS 5.x (`agsInit`/`agsDeInit` + `agsDriverExtensionsDX*`). RenderDoc BLOCKS the real
//! old-AGS driver-extension init (it uses a driver escape that conflicts with RenderDoc's D3D12 hooking)
//! which device-removes ER; a newer AGS 6.x DLL drops the 5.x export names so ER won't even load. This
//! stub exports EVERY name ER's real DLL exports (so imports bind) but does nothing real: `agsInit`
//! returns `AGS_NO_AMD_DRIVER_INSTALLED` (6) -- the exact result ER gets on an NVIDIA/Intel machine -- so
//! ER takes its well-tested non-AGS path (plain D3D12, no driver escape). Nothing here reads/writes ER's
//! structs, so there is no layout risk. Swapped in only for the capture; ER's real DLL is restored after.
//!
//! AGSReturnCode: SUCCESS=0, FAILURE=1, ..., NO_AMD_DRIVER_INSTALLED=6, EXTENSION_NOT_SUPPORTED=7.
//! Every export is `extern "C" -> i32` (AGSReturnCode); the x64 ABI passes args in registers, so a
//! no-arg stub validly ignores them and returns the code in eax. On failure ER never reads the out
//! params, so returning a code is sufficient.
#![allow(non_snake_case)]

const AGS_SUCCESS: i32 = 0;
const AGS_FAILURE: i32 = 1;
const AGS_NO_AMD_DRIVER_INSTALLED: i32 = 6;

macro_rules! ags_export {
    ($name:ident => $ret:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> i32 {
            $ret
        }
    };
}

// Lifecycle: report "no AMD driver" so ER falls back to plain D3D12; DeInit succeeds (may be called
// unconditionally at shutdown).
ags_export!(agsInit => AGS_NO_AMD_DRIVER_INSTALLED);
ags_export!(agsDeInit => AGS_SUCCESS);

// Everything else: ER should not call these once init reported no driver, but export them (so imports
// bind) returning FAILURE so any stray call is a clean no-op.
ags_export!(agsGetCrossfireGPUCount => AGS_FAILURE);
ags_export!(agsSetDisplayMode => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX12_Init => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX12_DeInit => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_Init => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_DeInit => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_BeginUAVOverlap => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_EndUAVOverlap => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_CreateBuffer => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_CreateTexture1D => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_CreateTexture2D => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_CreateTexture3D => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_GetMaxClipRects => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_IASetPrimitiveTopology => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_MultiDrawIndexedInstancedIndirect => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_MultiDrawIndexedInstancedIndirectCountIndirect => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_MultiDrawInstancedIndirect => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_MultiDrawInstancedIndirectCountIndirect => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_NotifyResourceBeginAllAccess => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_NotifyResourceEndAllAccess => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_NotifyResourceEndWrites => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_NumPendingAsyncCompileJobs => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_SetClipRects => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_SetDepthBounds => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_SetDiskShaderCacheEnabled => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_SetMaxAsyncCompileThreadCount => AGS_FAILURE);
ags_export!(agsDriverExtensionsDX11_SetViewBroadcastMasks => AGS_FAILURE);
