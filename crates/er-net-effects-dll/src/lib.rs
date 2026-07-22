//! Standalone keyboard-controlled network effect DLL.
//!
//! This crate owns the effect selector/hotkey feature that used to live inside
//! the product `er-effects-rs` DLL. It is shipped as `er_net_effects_dll.dll`
//! and can be listed as its own ME3 `[[natives]]` entry without pulling in the
//! product autoload/save/portrait/rendering dependencies.

#![allow(non_snake_case)]

#[cfg(windows)]
mod config;
#[cfg(windows)]
mod crash_telemetry;
#[cfg(windows)]
mod effects;
#[cfg(windows)]
mod input_suppression;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod present_overlay;
#[cfg(windows)]
mod telemetry;

#[cfg(windows)]
use std::sync::{Arc, Mutex, Once};

#[cfg(windows)]
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, PlayerIns},
    fd4::FD4TaskData,
};
#[cfg(windows)]
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};
#[cfg(windows)]
use windows::Win32::{Foundation::HINSTANCE, System::SystemServices::DLL_PROCESS_ATTACH};

#[cfg(windows)]
use crate::{effects::NetEffectsState, log::net_effects_log, telemetry::write_telemetry_throttled};

const DLL_MAIN_SUCCESS: i32 = 1;

#[cfg(windows)]
static START: Once = Once::new();

#[cfg(windows)]
fn state_or_recover(
    state: &Arc<Mutex<NetEffectsState>>,
) -> std::sync::MutexGuard<'_, NetEffectsState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(windows)]
fn wait_for_task_instance() -> &'static CSTaskImp {
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(_) => std::thread::yield_now(),
        }
    }
}

#[cfg(windows)]
fn player_runtime_ready(player: &PlayerIns) -> bool {
    player.chr_ins.chr_flags1c4.is_render_group_enabled()
        && player.chr_ins.chr_flags1c5.enable_render()
}

#[cfg(windows)]
fn spawn_game_task(state: Arc<Mutex<NetEffectsState>>) {
    let _ = std::thread::Builder::new()
        .name("er-net-effects-task".to_owned())
        .spawn(move || {
            net_effects_log(format_args!("game task thread waiting for CSTaskImp"));
            let task = wait_for_task_instance();
            net_effects_log(format_args!("game task registering FrameBegin tick"));
            task.run_recurring(
                move |_data: &FD4TaskData| {
                    let mut state = state_or_recover(&state);
                    state.game_task_ticks = state.game_task_ticks.saturating_add(1);
                    let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                        effects::set_runtime_ready(&mut state, false);
                        write_telemetry_throttled(&mut state, false);
                        return;
                    };
                    if !player_runtime_ready(player) {
                        effects::set_runtime_ready(&mut state, false);
                        write_telemetry_throttled(&mut state, true);
                        return;
                    }
                    effects::set_runtime_ready(&mut state, true);

                    effects::apply_pending_effect_work(player, &mut state);
                    effects::remove_requested_calls(player, &mut state);
                    effects::process_driver_command(player, &mut state);
                    effects::poll_live_effect_catalogs(player, &mut state);
                    effects::poll_live_effect_setting(player, &mut state);
                    effects::consume_effect_hotkeys(player, &mut state);
                    effects::publish_effect_selector_text(&mut state);
                    effects::refresh_call_status(player, &mut state);
                    effects::reapply_expired_enabled_calls(player, &mut state);
                    write_telemetry_throttled(&mut state, true);
                },
                CSTaskGroupIndex::FrameBegin,
            );
        });
}

#[cfg(windows)]
fn install() {
    config::init_runtime_config();
    log::reset_log_file();
    crash_telemetry::install_handler();
    net_effects_log(format_args!(
        "er-net-effects attach: standalone keyboard-controlled SpEffect selector; network_sync={} config={}",
        config::runtime_config().network_sync,
        config::runtime_config().config_path.display()
    ));
    effects::ensure_effect_hotkey_hook();
    present_overlay::install_present_overlay_hook();
    let state = Arc::new(Mutex::new(NetEffectsState::new()));
    spawn_game_task(state);
}

#[cfg(windows)]
#[unsafe(no_mangle)]
/// # Safety
/// Standard Windows `DllMain`; on attach it only starts an installer thread.
pub unsafe extern "system" fn DllMain(
    _module: HINSTANCE,
    reason: u32,
    _reserved: *mut core::ffi::c_void,
) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        START.call_once(|| {
            crash_telemetry::set_module_base(_module);
            let _ = std::thread::Builder::new()
                .name("er-net-effects-install".to_owned())
                .spawn(install);
        });
    }
    DLL_MAIN_SUCCESS
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn er_net_effects_dll_host_stub() -> i32 {
    DLL_MAIN_SUCCESS
}
