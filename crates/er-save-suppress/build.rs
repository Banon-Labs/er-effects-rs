//! Generates every prologue this crate byte-checks, from named `iced-x86` instructions.
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
    Assemble, Image, PrologueSpec, Shape, generate, mov_r32_rm32, mov_r64_rm64,
    mov_rax_rip_absolute, rex_push, xor_r32_rm32,
};

const SUPPORT: &str = "../../build-support/prologue_build.rs";

/// The MSVC `/GS` cookie slot every one of these frames seeds with `-2` before anything else.
const GS_COOKIE_SEED: i32 = -2;
/// `GLOBAL_CSGameMan` at its 1.17 address, the singleton the quit-phase settle loads RIP-relative.
/// Naming the absolute address lets iced compute the displacement for the VA being assembled at,
/// instead of a transcribed `+0x36f21b1` that means nothing on its own.
///
/// 1.16.2 had it at `0x143d69918`; 1.17 moved it `+0x4070` with the rest of `.data`
/// (`GAME_MAN_SINGLETON_RVA` in the tracked data map, 136 of 137 references agreeing). Only
/// `QUIT_PHASE_SETTLE_SIG` uses it, and that signature is what decides whether the settle patch
/// is applied at all -- see the spec for why the address has to be the 1.17 one.
const GAME_MAN_SINGLETON_VA: u64 = 0x143d6d988;
/// `GameMan+0xbc4`, the quit-to-title phase field.
const GAME_MAN_QUIT_PHASE_OFFSET: i64 = 0xbc4;
/// The phase value the settle function tests for.
const QUIT_PHASE_WAITING: i32 = 2;

/// `SaveLoad2::SLSystemImpl::EnqueueSaveJob` -- every save request funnels through it.
const SL_ENQUEUE_SAVE_JOB_VA: u64 = 0x140e6fb50;
/// `SaveLoad2::SLSystemImpl::PollSaveStatus`.
const SL_POLL_SAVE_STATUS_VA: u64 = 0x140e6e430;
/// The only code that moves `GameMan+0xbc4` from 2 to 3, at its 1.17 address. 1.16.2
/// `FUN_14067a980`; 1.17 `0x14067b7d0`, `+0xe50`, the whole-region delta for this part of `.text`.
const QUIT_PHASE_SETTLE_VA: u64 = 0x14067b7d0;
/// Where it jumps when the phase is not 2 -- the branch is inside the checked window, so its
/// destination is part of the prologue. Still exactly `+0x1a` from the entry in 1.17.
const QUIT_PHASE_SETTLE_NOT_WAITING_VA: u64 = 0x14067b7ea;
/// `SaveLoad2::SLSystemImpl::ReleaseRequest`.
const SL_RELEASE_REQUEST_VA: u64 = 0x140e6f200;
/// `FUN_14067b940`, the combined character+system save lane.
const SAVE_DISPATCH_COMBINED_VA: u64 = 0x14067b940;
/// `FUN_14067b750`, the character-slot-only lane.
const SAVE_DISPATCH_CHAR_VA: u64 = 0x14067b750;
/// `FUN_14067b570`, the system-slot-only lane.
const SAVE_DISPATCH_SYSTEM_VA: u64 = 0x14067b570;
/// `FUN_14067dc00`, the character serializer.
const SAVE_SERIALIZE_CHAR_VA: u64 = 0x14067dc00;
/// `FUN_142413860`, the full save-container rebuild.
const SAVE_WRITE_FULL_REBUILD_VA: u64 = 0x142413860;
/// `FUN_1424142e0`, the per-block in-place patcher.
const SAVE_WRITE_IN_PLACE_VA: u64 = 0x1424142e0;
/// `FUN_14240fd70`, `SaveLoad2::SLSaveSession`'s job body.
const SL_SAVE_JOB_BODY_VA: u64 = 0x14240fd70;

/// How much of each write branch is compared. Both open with the same seven-instruction
/// multi-push prologue and diverge only at the frame-pointer `lea`, so the window has to reach
/// past byte 12 or one signature would match both functions.
const SAVE_WRITE_CHECKED_BYTES: usize = 24;
/// The character serializer's window stops one byte inside its `/GS` cookie store.
const SAVE_SERIALIZE_CHAR_CHECKED_BYTES: usize = 24;

/// The seven callee-saved pushes both save-write branches open with.
fn save_write_pushes(asm: &mut CodeAssembler) -> Result<(), iced_x86::IcedError> {
    rex_push(asm, rbp)?;
    asm.push(rsi)?;
    asm.push(rdi)?;
    asm.push(r12)?;
    asm.push(r13)?;
    asm.push(r14)?;
    asm.push(r15)
}

fn main() {
    prologue_build::declare_rerun(SUPPORT);

    generate(
        &[
            (
                PrologueSpec {
                    name: "SL_ENQUEUE_SAVE_JOB_SIG",
                    doc: "`push rbx/rsi/rdi; sub rsp,0x50; <GS cookie>; mov esi,edx; mov rbx,rcx`.\n\
                          Note the two-byte `40 53` push -- a redundant REX prefix MSVC emits here.\n\
                          Decodes to whole instructions well past MinHook's 5-byte relocation\n\
                          window and contains no relative branch, so it is safe to patch.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SL_ENQUEUE_SAVE_JOB_VA,
                    take: 0,
                    pin: &[
                        0x40, 0x53, 0x56, 0x57, 0x48, 0x83, 0xEC, 0x50, 0x48, 0xC7, 0x44, 0x24,
                        0x30, 0xFE, 0xFF, 0xFF, 0xFF, 0x8B, 0xF2, 0x48, 0x8B, 0xD9,
                    ],
                },
                (|asm| {
                    rex_push(asm, rbx)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x50)?;
                    asm.mov(qword_ptr(rsp + 0x30), GS_COOKIE_SEED)?;
                    mov_r32_rm32(asm, Register::ESI, Register::EDX)?;
                    mov_r64_rm64(asm, Register::RBX, Register::RCX)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SL_POLL_SAVE_STATUS_SIG",
                    doc: "`push rdi; sub rsp,0x70; <GS cookie>; mov [rsp+0x88],rbx`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SL_POLL_SAVE_STATUS_VA,
                    take: 0,
                    pin: &[
                        0x40, 0x57, 0x48, 0x83, 0xEC, 0x70, 0x48, 0xC7, 0x44, 0x24, 0x28, 0xFE,
                        0xFF, 0xFF, 0xFF, 0x48, 0x89, 0x9C, 0x24, 0x88, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    rex_push(asm, rdi)?;
                    asm.sub(rsp, 0x70)?;
                    asm.mov(qword_ptr(rsp + 0x28), GS_COOKIE_SEED)?;
                    asm.mov(qword_ptr(rsp + 0x88), rbx)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "QUIT_PHASE_SETTLE_SIG",
                    doc: "`mov rax,[rip+..]; cmp dword [rax+0xbc4],2; jne`. The whole function is\n\
                          `if (bc4 == 2) bc4 = 3;`, so the checked window covers its only test.\n\
                          \n\
                          Assembled for 1.17. The instructions, the field offset and the tested\n\
                          value are all unchanged from 1.16.2 -- only the RIP displacement moved,\n\
                          because both the function and the singleton did. That is three bytes,\n\
                          and until 2026-08-30 they were 1.16.2's: the signature failed at its\n\
                          own site on the installed game, the settle patch declined to apply, and\n\
                          nothing said so.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing1170,
                    va: QUIT_PHASE_SETTLE_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8B, 0x05, 0xB1, 0x21, 0x6F, 0x03, 0x83, 0xB8, 0xC4, 0x0B, 0x00,
                        0x00, 0x02, 0x75, 0x0A,
                    ],
                },
                (|asm| {
                    mov_rax_rip_absolute(asm, GAME_MAN_SINGLETON_VA)?;
                    asm.cmp(
                        dword_ptr(rax + GAME_MAN_QUIT_PHASE_OFFSET),
                        QUIT_PHASE_WAITING,
                    )?;
                    asm.jne(QUIT_PHASE_SETTLE_NOT_WAITING_VA)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SL_RELEASE_REQUEST_SIG",
                    doc: "`mov [rsp+0x10],rbp; mov [rsp+0x18],rsi; push rdi; sub rsp,0x20;\n\
                          xor ebp,ebp; mov rdi,rcx; cmp [rcx+0x28],rbp`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SL_RELEASE_REQUEST_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x89, 0x6C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x57, 0x48,
                        0x83, 0xEC, 0x20, 0x33, 0xED, 0x48, 0x8B, 0xF9, 0x48, 0x39, 0x69, 0x28,
                    ],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 0x10), rbp)?;
                    asm.mov(qword_ptr(rsp + 0x18), rsi)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x20)?;
                    xor_r32_rm32(asm, Register::EBP, Register::EBP)?;
                    mov_r64_rm64(asm, Register::RDI, Register::RCX)?;
                    asm.cmp(qword_ptr(rcx + 0x28), rbp)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_DISPATCH_COMBINED_SIG",
                    doc: "`FUN_14067b940`: `mov rax,rsp; mov [rax+0x18],r8b; push rdi/r12/r13/r14/r15;\n\
                          sub rsp,0xb0`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_DISPATCH_COMBINED_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8B, 0xC4, 0x44, 0x88, 0x40, 0x18, 0x57, 0x41, 0x54, 0x41, 0x55,
                        0x41, 0x56, 0x41, 0x57, 0x48, 0x81, 0xEC, 0xB0, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    mov_r64_rm64(asm, Register::RAX, Register::RSP)?;
                    asm.mov(byte_ptr(rax + 0x18), r8b)?;
                    asm.push(rdi)?;
                    asm.push(r12)?;
                    asm.push(r13)?;
                    asm.push(r14)?;
                    asm.push(r15)?;
                    asm.sub(rsp, 0xb0)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_DISPATCH_CHAR_SIG",
                    doc: "`FUN_14067b750`: `mov [rsp+0x20],rbx; push rdi/r12/r15; sub rsp,0x30;\n\
                          movzx r15d,r8b; movzx r12d,dl; mov edi,ecx`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_DISPATCH_CHAR_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x89, 0x5C, 0x24, 0x20, 0x57, 0x41, 0x54, 0x41, 0x57, 0x48, 0x83,
                        0xEC, 0x30, 0x45, 0x0F, 0xB6, 0xF8, 0x44, 0x0F, 0xB6, 0xE2, 0x8B, 0xF9,
                    ],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 0x20), rbx)?;
                    asm.push(rdi)?;
                    asm.push(r12)?;
                    asm.push(r15)?;
                    asm.sub(rsp, 0x30)?;
                    asm.movzx(r15d, r8b)?;
                    asm.movzx(r12d, dl)?;
                    mov_r32_rm32(asm, Register::EDI, Register::ECX)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_DISPATCH_SYSTEM_SIG",
                    doc: "`FUN_14067b570`: `mov rax,rsp; push rdi; sub rsp,0xa0; <GS cookie>;\n\
                          mov [rax+8],rbx`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_DISPATCH_SYSTEM_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x8B, 0xC4, 0x57, 0x48, 0x81, 0xEC, 0xA0, 0x00, 0x00, 0x00, 0x48,
                        0xC7, 0x44, 0x24, 0x20, 0xFE, 0xFF, 0xFF, 0xFF, 0x48, 0x89, 0x58, 0x08,
                    ],
                },
                (|asm| {
                    mov_r64_rm64(asm, Register::RAX, Register::RSP)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0xa0)?;
                    asm.mov(qword_ptr(rsp + 0x20), GS_COOKIE_SEED)?;
                    asm.mov(qword_ptr(rax + 8), rbx)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_SERIALIZE_CHAR_SIG",
                    doc: "`FUN_14067dc00`: `push rbp/rbx/rsi/rdi; lea rbp,[rsp-0x58];\n\
                          sub rsp,0x158; <GS cookie>`. The window stops one byte inside the\n\
                          cookie store, which is why the assembled sequence is longer.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_SERIALIZE_CHAR_VA,
                    take: SAVE_SERIALIZE_CHAR_CHECKED_BYTES,
                    pin: &[
                        0x40, 0x55, 0x53, 0x56, 0x57, 0x48, 0x8D, 0x6C, 0x24, 0xA8, 0x48, 0x81,
                        0xEC, 0x58, 0x01, 0x00, 0x00, 0x48, 0xC7, 0x45, 0xA0, 0xFE, 0xFF, 0xFF,
                    ],
                },
                (|asm| {
                    rex_push(asm, rbp)?;
                    asm.push(rbx)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.lea(rbp, qword_ptr(rsp - 0x58))?;
                    asm.sub(rsp, 0x158)?;
                    asm.mov(qword_ptr(rbp - 0x60), GS_COOKIE_SEED)?;
                    Ok(())
                }) as Assemble,
            ),
        ],
        "generated_save_suppress_prologues.rs",
    );

    generate(
        &[
            (
                PrologueSpec {
                    name: "SAVE_WRITE_FULL_REBUILD_SIG",
                    doc: "`FUN_142413860`: the shared seven pushes, then `lea rbp,[rsp-0x60]`\n\
                          and a 0x160 frame. The `lea` is where it diverges from the in-place\n\
                          patcher, so a shorter window would match both functions.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_WRITE_FULL_REBUILD_VA,
                    take: SAVE_WRITE_CHECKED_BYTES,
                    pin: &[
                        0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
                        0x48, 0x8D, 0x6C, 0x24, 0xA0, 0x48, 0x81, 0xEC, 0x60, 0x01, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    save_write_pushes(asm)?;
                    asm.lea(rbp, qword_ptr(rsp - 0x60))?;
                    asm.sub(rsp, 0x160)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SAVE_WRITE_IN_PLACE_SIG",
                    doc: "`FUN_1424142e0`: the shared seven pushes, then `lea rbp,[rsp-0xd0]`\n\
                          and a 0x1d0 frame. The window stops inside that `sub`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SAVE_WRITE_IN_PLACE_VA,
                    take: SAVE_WRITE_CHECKED_BYTES,
                    pin: &[
                        0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
                        0x48, 0x8D, 0xAC, 0x24, 0x30, 0xFF, 0xFF, 0xFF, 0x48, 0x81, 0xEC, 0xD0,
                    ],
                },
                (|asm| {
                    save_write_pushes(asm)?;
                    asm.lea(rbp, qword_ptr(rsp - 0xd0))?;
                    asm.sub(rsp, 0x1d0)?;
                    Ok(())
                }) as Assemble,
            ),
        ],
        "generated_save_write_branch_prologues.rs",
    );

    generate(
        &[(
            PrologueSpec {
                name: "SL_SAVE_JOB_BODY_SIG",
                doc: "`FUN_14240fd70`: `mov rax,rsp; push rbp/rdi/r12/r14/r15;\n\
                      lea rbp,[rax-0x5f]; sub rsp,0xb0`.",
                visibility: "",
                shape: Shape::Slice,
                image: Image::EldenRing,
                va: SL_SAVE_JOB_BODY_VA,
                take: 0,
                pin: &[
                    0x48, 0x8B, 0xC4, 0x55, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D,
                    0x68, 0xA1, 0x48, 0x81, 0xEC, 0xB0, 0x00, 0x00, 0x00,
                ],
            },
            (|asm| {
                mov_r64_rm64(asm, Register::RAX, Register::RSP)?;
                asm.push(rbp)?;
                asm.push(rdi)?;
                asm.push(r12)?;
                asm.push(r14)?;
                asm.push(r15)?;
                asm.lea(rbp, qword_ptr(rax - 0x5f))?;
                asm.sub(rsp, 0xb0)?;
                Ok(())
            }) as Assemble,
        )],
        "generated_save_job_completion_prologues.rs",
    );
}
