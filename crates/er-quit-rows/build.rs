//! Generates every prologue the product DLL byte-checks, from named `iced-x86` instructions.
//!
//! See `build-support/prologue_build.rs` for why these are generated rather than hand-typed and
//! for what verifies the result.

#[allow(dead_code)]
mod prologue_build {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/prologue_build.rs"
    ));
}

use iced_x86::Register;
use iced_x86::code_asm::*;
use prologue_build::{
    Assemble, Image, PrologueSpec, Shape, generate, mov_r32_mem, mov_r64_rm64,
    mov_rax_rip_absolute, rex_push,
};

const SUPPORT: &str = "../../build-support/prologue_build.rs";

/// `GLOBAL_CSGameMan`, at its 1.17 address. The two retractions load it RIP-relative; naming the
/// absolute address lets iced compute each site's displacement instead of transcribing it.
///
/// 1.16.2 had it at `0x143d69918`; 1.17 moved it `+0x4070`, along with most of `.data`. That is
/// carried in `docs/recon/rva-map-1162-to-1170.data.tsv` as `GAME_MAN_SINGLETON_RVA` on 136 of
/// 137 independent references, and confirmed here by reading the 1.17 image: all three sites that
/// load it (`0x140679560`, `0x140679590`, `0x14067ae80`) resolve to `0x143d6d988`.
///
/// It is spelled at the 1.17 address because the constants it produces are compared against the
/// bytes of the running game. See [`Image::EldenRing1170`] for why a mapped RVA does not make a
/// 1.16.2 signature usable.
const GAME_MAN_SINGLETON_VA: u64 = 0x143d6d988;
/// `GameMan+0xb72` / `+0xb73`, the two save-request flags the retractions clear.
const GAME_MAN_SAVE_REQUEST_B72_OFFSET: i64 = 0xb72;
const GAME_MAN_SAVE_REQUEST_B73_OFFSET: i64 = 0xb73;

/// `CS::MenuJob::EmitResult`.
const MENU_JOB_EMIT_RESULT_VA: u64 = 0x140746e80;
/// The game's own retractions of the two save-request flags, at their 1.17 addresses.
///
/// 1.16.2 `FUN_140678740` / `FUN_140678710`; 1.17 `0x140679590` / `0x140679560`, both `+0xe50`,
/// which is the whole-region delta for this part of `.text` and agrees with three unanimous
/// caller votes each. Read out of `eldenring-deobf-1.17.bin`, the two bodies are still exactly
/// `mov rax,[rip+disp]; mov byte [rax+0xb7x],0; ret`, and the field offsets `+0xb72`/`+0xb73` are
/// unchanged -- only `disp` moved, because the singleton did.
///
/// The site address matters as much as the target: a RIP displacement is the distance between
/// them, so generating the signature at the 1.16.2 site would encode the wrong four bytes even
/// with the target corrected.
const SAVE_REQUEST_RETRACT_B72_VA: u64 = 0x140679590;
const SAVE_REQUEST_RETRACT_B73_VA: u64 = 0x140679560;
/// The `CS::MessageBoxBuilder` recipe, lifted from the native Yes/No confirm `FUN_1407b73d0`.
const MSGBOX_BUILDER_CTOR_VA: u64 = 0x1407af730;
const MSGBOX_ADD_YES_VA: u64 = 0x1407b1c70;
const MSGBOX_ADD_NO_VA: u64 = 0x1407b1900;
const MSGBOX_DEFAULT_LAST_VA: u64 = 0x1407b1b60;
const MSGBOX_FINALIZE_VA: u64 = 0x1407b10f0;
const MSGBOX_DTOR_VA: u64 = 0x1407b0140;
/// `MessageBoxBuilder+0x10f0` is the button count and `+0x28` the default index; `default_last`
/// is the whole two-field body `default = count - 1`.
const MSGBOX_BUILDER_BUTTON_COUNT_OFFSET: i64 = 0x10f0;
const MSGBOX_BUILDER_DEFAULT_INDEX_OFFSET: i64 = 0x28;

fn main() {
    prologue_build::declare_rerun(SUPPORT);

    generate(
        &[
            (
                PrologueSpec {
                    name: "MENU_JOB_EMIT_RESULT_SIG",
                    doc: "Prologue of `CS::MenuJob::EmitResult`:\n\
                          `mov [rsp+0x10],rdx; push rbx; sub rsp,0x80`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MENU_JOB_EMIT_RESULT_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x89, 0x54, 0x24, 0x10, 0x53, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00,
                        0x00,
                    ],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 0x10), rdx)?;
                    asm.push(rbx)?;
                    asm.sub(rsp, 0x80)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_REQUEST_RETRACT_B72_SIG",
                    doc: "WHOLE BODY of the `+0xb72` retraction (1.16.2 `FUN_140678740`, 1.17\n\
                          `0x140679590`): load the GameMan singleton, store 0 into `+0xb72`,\n\
                          return. Verified before the call: if the bytes ever differ, the\n\
                          address means something else in that build and the retraction is\n\
                          skipped rather than fired blind at unknown code.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing1170,
                    va: SAVE_REQUEST_RETRACT_B72_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8B, 0x05, 0xF1, 0x43, 0x6F, 0x03, 0xC6, 0x80, 0x72, 0x0B, 0x00,
                        0x00, 0x00, 0xC3,
                    ],
                },
                (|asm| {
                    mov_rax_rip_absolute(asm, GAME_MAN_SINGLETON_VA)?;
                    asm.mov(byte_ptr(rax + GAME_MAN_SAVE_REQUEST_B72_OFFSET), 0)?;
                    asm.ret()?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_REQUEST_RETRACT_B73_SIG",
                    doc: "WHOLE BODY of the `+0xb73` retraction (1.16.2 `FUN_140678710`, 1.17\n\
                          `0x140679560`), the same three instructions against `+0xb73`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing1170,
                    va: SAVE_REQUEST_RETRACT_B73_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8B, 0x05, 0x21, 0x44, 0x6F, 0x03, 0xC6, 0x80, 0x73, 0x0B, 0x00,
                        0x00, 0x00, 0xC3,
                    ],
                },
                (|asm| {
                    mov_rax_rip_absolute(asm, GAME_MAN_SINGLETON_VA)?;
                    asm.mov(byte_ptr(rax + GAME_MAN_SAVE_REQUEST_B73_OFFSET), 0)?;
                    asm.ret()?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_BUILDER_CTOR_SIG",
                    doc: "`ctor(rcx=builder, rdx=ctx, r8=prompt MenuString*, r9=&mode_i32,\n\
                          [rsp+0x28]=0u8)`: `push rbp/rsi/rdi; sub rsp,0x80`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_BUILDER_CTOR_VA,
                    take: 0,
                    pin: &[
                        0x40, 0x55, 0x56, 0x57, 0x48, 0x81, 0xec, 0x80, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    rex_push(asm, rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x80)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_ADD_YES_SIG",
                    doc: "`add_yes(rcx=builder, rdx=&SaveFlowYesButtonDesc) -> builder`:\n\
                          `mov r11,rsp; push rdi; sub rsp,0x90`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_ADD_YES_VA,
                    take: 0,
                    pin: &[
                        0x4c, 0x8b, 0xdc, 0x57, 0x48, 0x81, 0xec, 0x90, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    mov_r64_rm64(asm, Register::R11, Register::RSP)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x90)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_ADD_NO_SIG",
                    doc: "`add_no(rcx=builder) -> builder`: `push rdi; sub rsp,0xa0`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_ADD_NO_VA,
                    take: 0,
                    pin: &[0x40, 0x57, 0x48, 0x81, 0xec, 0xa0, 0x00, 0x00, 0x00],
                },
                (|asm| {
                    rex_push(asm, rdi)?;
                    asm.sub(rsp, 0xa0)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_DEFAULT_LAST_SIG",
                    doc: "WHOLE BODY of `default_last(rcx=builder) -> builder`:\n\
                          `builder->default_index = builder->button_count - 1; return builder;`.\n\
                          That is why add order encodes the default.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_DEFAULT_LAST_VA,
                    take: 0,
                    pin: &[
                        0x8b, 0x81, 0xf0, 0x10, 0x00, 0x00, 0xff, 0xc8, 0x89, 0x41, 0x28, 0x48,
                        0x8b, 0xc1, 0xc3,
                    ],
                },
                (|asm| {
                    mov_r32_mem(
                        asm,
                        Register::EAX,
                        Register::RCX,
                        MSGBOX_BUILDER_BUTTON_COUNT_OFFSET,
                    )?;
                    asm.dec(eax)?;
                    asm.mov(dword_ptr(rcx + MSGBOX_BUILDER_DEFAULT_INDEX_OFFSET), eax)?;
                    mov_r64_rm64(asm, Register::RAX, Register::RCX)?;
                    asm.ret()?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_FINALIZE_SIG",
                    doc: "`finalize(rcx=builder, rdx=&job_slot, r8b=0) -> &job_slot`:\n\
                          `mov r11,rsp; push rsi/rdi/r14; sub rsp,0x130`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_FINALIZE_VA,
                    take: 0,
                    pin: &[
                        0x4c, 0x8b, 0xdc, 0x56, 0x57, 0x41, 0x56, 0x48, 0x81, 0xec, 0x30, 0x01,
                        0x00, 0x00,
                    ],
                },
                (|asm| {
                    mov_r64_rm64(asm, Register::R11, Register::RSP)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.push(r14)?;
                    asm.sub(rsp, 0x130)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SYSTEM_QUIT_MSGBOX_DTOR_SIG",
                    doc: "`dtor(rcx=builder)`: `mov [rsp+8],rcx; push rdi; sub rsp,0x30`.",
                    visibility: "pub(crate)",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: MSGBOX_DTOR_VA,
                    take: 0,
                    pin: &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x30],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 8), rcx)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x30)?;
                    Ok(())
                }) as Assemble,
            ),
        ],
        "generated_autoload_state_prologues.rs",
    );
}
