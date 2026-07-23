#![cfg(windows)]

use std::{
    env,
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, EquipParamProtector, SoloParamRepository},
    fd4::FD4TaskData,
    param::EQUIP_PARAM_PROTECTOR_ST,
};
use fromsoftware_shared::{FromStatic, SharedTaskImpExt};

const DLL_MAIN_SUCCESS: i32 = 1;
const DLL_PROCESS_ATTACH: u32 = 1;
const PROTECTOR_PARAM_INDEX: usize = 1;
const PRIMARY_RES_CAP_INDEX: usize = 0;
const NO_PATCH_ATTEMPTS: u32 = 0;
const FIRST_PATCH_ATTEMPT: u32 = 1;
const PATCH_RETRY_LOG_INTERVAL: u32 = 100_000;
const PATCH_RETRY_REMAINDER: u32 = 0;
const PATCHED_ROW_INCREMENT: usize = 1;
const NO_PATCHED_ROWS: usize = 0;
const HIDDEN_MODEL_ID: u16 = 0;
const HIDDEN_MODEL_GENDER: u8 = 3;
const HEAD_MODEL_CATEGORY: u8 = 5;
const BODY_MODEL_CATEGORY: u8 = 2;
const ARM_MODEL_CATEGORY: u8 = 1;
const LEG_MODEL_CATEGORY: u8 = 6;
const CLEARED_SEX_VARIANT_HIDE_MASK: u8 = 0;

static START_PATCH_TASK: AtomicBool = AtomicBool::new(false);
static PATCH_APPLIED: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "system" fn DllMain(
    _hmodule: *mut c_void,
    reason: u32,
    _reserved: *mut c_void,
) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }

    if START_PATCH_TASK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        std::thread::spawn(spawn_param_patch_task);
    }

    DLL_MAIN_SUCCESS
}

fn spawn_param_patch_task() {
    write_runtime_log("patch task started");
    let mut attempts = NO_PATCH_ATTEMPTS;
    let cs_task = loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => break instance,
            Err(error) => {
                attempts = attempts.saturating_add(FIRST_PATCH_ATTEMPT);
                if attempts == FIRST_PATCH_ATTEMPT
                    || attempts % PATCH_RETRY_LOG_INTERVAL == PATCH_RETRY_REMAINDER
                {
                    write_runtime_log(&format!(
                        "waiting for CSTaskImp attempt={attempts} error={error:?}"
                    ));
                }
                std::thread::yield_now();
            }
        }
    };
    write_runtime_log(&format!("found CSTaskImp after {attempts} retry attempts"));

    cs_task.run_recurring(
        move |_: &FD4TaskData| {
            if PATCH_APPLIED.load(Ordering::Acquire) {
                return;
            }

            let Some(report) = try_patch_loaded_protectors() else {
                return;
            };

            write_runtime_log(&format!(
                "patched EquipParamProtector visual rows: {}",
                report.eligible_slot_rows
            ));
            PATCH_APPLIED.store(true, Ordering::Release);
        },
        CSTaskGroupIndex::FrameBegin,
    );
}

fn try_patch_loaded_protectors() -> Option<PatchReport> {
    // SAFETY: This recurring task runs on the game's task/main thread. That is
    // the same exclusivity boundary fromsoftware-rs documents for mutating
    // singleton game objects.
    let repository = unsafe { SoloParamRepository::instance_mut().ok()? };
    let holder = repository.solo_param_holders.get(PROTECTOR_PARAM_INDEX)?;
    holder.get_res_cap(PRIMARY_RES_CAP_INDEX)?;

    let mut report = PatchReport::default();
    for (_row_id, row) in repository.rows_mut::<EquipParamProtector>() {
        if patch_protector_row(row) {
            report.eligible_slot_rows += PATCHED_ROW_INCREMENT;
        }
    }

    (report.eligible_slot_rows > NO_PATCHED_ROWS).then_some(report)
}

fn patch_protector_row(row: &mut EQUIP_PARAM_PROTECTOR_ST) -> bool {
    let mut eligible = false;
    if row.head_equip() {
        row.set_equip_model_id(HIDDEN_MODEL_ID);
        row.set_equip_model_category(HEAD_MODEL_CATEGORY);
        row.set_equip_model_gender(HIDDEN_MODEL_GENDER);
        eligible = true;
    }
    if row.body_equip() {
        row.set_equip_model_id(HIDDEN_MODEL_ID);
        row.set_equip_model_category(BODY_MODEL_CATEGORY);
        row.set_equip_model_gender(HIDDEN_MODEL_GENDER);
        eligible = true;
    }
    if row.arm_equip() {
        row.set_equip_model_id(HIDDEN_MODEL_ID);
        row.set_equip_model_category(ARM_MODEL_CATEGORY);
        row.set_equip_model_gender(HIDDEN_MODEL_GENDER);
        eligible = true;
    }
    if row.leg_equip() {
        row.set_equip_model_id(HIDDEN_MODEL_ID);
        row.set_equip_model_category(LEG_MODEL_CATEGORY);
        row.set_equip_model_gender(HIDDEN_MODEL_GENDER);
        eligible = true;
    }

    if eligible {
        clear_visual_hide_masks(row);
    }

    eligible
}

fn clear_visual_hide_masks(row: &mut EQUIP_PARAM_PROTECTOR_ST) {
    row.set_use_face_scale(false);
    row.set_invisible_flag48(false);
    row.set_invisible_flag49(false);
    row.set_invisible_flag50(false);
    row.set_invisible_flag51(false);
    row.set_invisible_flag52(false);
    row.set_invisible_flag53(false);
    row.set_invisible_flag54(false);
    row.set_invisible_flag55(false);
    row.set_invisible_flag56(false);
    row.set_invisible_flag57(false);
    row.set_invisible_flag58(false);
    row.set_invisible_flag59(false);
    row.set_invisible_flag60(false);
    row.set_invisible_flag61(false);
    row.set_invisible_flag62(false);
    row.set_invisible_flag63(false);
    row.set_invisible_flag64(false);
    row.set_invisible_flag65(false);
    row.set_invisible_flag66(false);
    row.set_invisible_flag67(false);
    row.set_invisible_flag68(false);
    row.set_invisible_flag69(false);
    row.set_invisible_flag70(false);
    row.set_invisible_flag71(false);
    row.set_invisible_flag72(false);
    row.set_invisible_flag73(false);
    row.set_invisible_flag74(false);
    row.set_invisible_flag75(false);
    row.set_invisible_flag76(false);
    row.set_invisible_flag77(false);
    row.set_invisible_flag78(false);
    row.set_invisible_flag79(false);
    row.set_invisible_flag80(false);
    row.set_invisible_flag_sex_ver00(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver01(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver02(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver03(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver04(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver05(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver06(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver07(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver08(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver09(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver10(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver11(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver12(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver13(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver14(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver15(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver16(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver17(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver18(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver19(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver20(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver21(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver22(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver23(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver24(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver25(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver26(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver27(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver28(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver29(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver30(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver31(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver32(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver33(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver34(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver35(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver36(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver37(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver38(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver39(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver40(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver41(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver42(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver43(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver44(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver45(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver46(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver47(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver48(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver49(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver50(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver51(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver52(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver53(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver54(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver55(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver56(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver57(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver58(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver59(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver60(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver61(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver62(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver63(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver64(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver65(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver66(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver67(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver68(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver69(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver70(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver71(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver72(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver73(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver74(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver75(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver76(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver77(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver78(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver79(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver80(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver81(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver82(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver83(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver84(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver85(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver86(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver87(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver88(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver89(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver90(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver91(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver92(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver93(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver94(CLEARED_SEX_VARIANT_HIDE_MASK);
    row.set_invisible_flag_sex_ver95(CLEARED_SEX_VARIANT_HIDE_MASK);
}

fn write_runtime_log(message: &str) {
    let Some(path) = runtime_log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{:?} {message}", SystemTime::now());
    }
}

fn runtime_log_path() -> Option<PathBuf> {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("MushroomMan").join("mushroom_man.log"))
}

#[derive(Default)]
struct PatchReport {
    eligible_slot_rows: usize,
}
