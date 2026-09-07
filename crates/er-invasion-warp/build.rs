//! Generates every prologue this shell byte-checks, from named `iced-x86` instructions.
//!
//! Two modules are covered: Elden Ring itself (`announce.rs`, `place_name.rs`) and Seamless
//! Co-op's `ersc.dll` (`local_invasion_filter.rs`). See `build-support/prologue_build.rs`.

#[allow(dead_code)]
mod prologue_build {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../build-support/prologue_build.rs"
    ));
}

use iced_x86::Register;
use iced_x86::code_asm::*;
use prologue_build::{Image, PrologueSpec, Shape, generate, mov_r64_mem, rex_push};

const SUPPORT: &str = "../../build-support/prologue_build.rs";

/// `CS::FeSystemAnnounceView::Update`.
const ANNOUNCE_UPDATE_VA: u64 = 0x1408c47c0;
/// How much of `Update`'s opening the check reads. It deliberately stops PART WAY through the
/// `movaps` that spills `xmm6`, which is why the assembled sequence is longer than the constant.
const ANNOUNCE_UPDATE_CHECKED_BYTES: usize = 8;
/// `FUN_140d10b60(MsgRepositoryImp*, id) -> wchar_t*`, the `PlaceName` getter.
const PLACE_NAME_LOOKUP_VA: u64 = 0x140d10b60;

// ---------------------------------------------------------------------------------------------
// Seamless Co-op's `ersc.dll`, at its preferred base `0x180000000`
//
// ONE build is described here -- the one `ERSC_SUPPORTED_VERSION` names -- because every address,
// field offset and state code below was measured against that build and none of them survives a
// Seamless update. `ersc.dll` is third-party: the user installs and updates it on their own
// schedule and may downgrade, and the launcher leaves the build it replaced in `_SeamlessCoop/`,
// so a file is identified by its CONTENT before any pin is compared against it.
// `local_invasion_filter.rs` refuses AT RUNTIME on a module whose invade action does not
// byte-match -- see `ersc::Abi` there.
//
// # These pins are longer than a prologue, on purpose
//
// The two option actions open with fourteen IDENTICAL bytes, and five different actions share
// them (`0x24ef0`, `0x257d0`, `0x25850`, `0x258d0`, `0x259d0`, measured 2026-09-06 over the
// supported build's plaintext `.text`). A fourteen-byte check therefore proves "an option action
// lives here", not "THE invade action lives here" -- and this module's failure mode is calling
// the wrong one and cancelling other players' invasions. So each option-action pin runs through
// the state WRITE, which is the instruction that makes an action what it is. What that covers,
// all in one check:
//
//   the session load offset        `mov rdi,[rcx+0x58]`
//   the state field and its idle   `cmp dword [rdi+STATE], IDLE`
//   the mutex sub-object offset    `lea rsi,[rdi+MUTEX]`
//   the mutex lock/unlock pair     `call <lock>` (its rel32 is a fixed constant of the build)
//   the guard field and its poison `cmp dword [rdi+GUARD], 0x7fffffff`
//   the action code                `mov dword [rdi+STATE], CODE`
//
// Every pin below occurs EXACTLY ONCE in the supported build, which is what makes the runtime
// gate an identification rather than a guess. `show` is the one exception and is deliberately not
// used to identify anything: it is also the one function this module HOOKS, so its bytes stop
// being the shipped bytes once the detour is in.

/// Entry points of the supported build. See `local_invasion_filter.rs` for the evidence that
/// identifies each one; none of them was taken from a prologue match alone.
const ERSC201_SHOW_VA: u64 = 0x1800241a0;
const ERSC201_INVADE_ACTION_VA: u64 = 0x180025850;
const ERSC201_CANCEL_ACTION_VA: u64 = 0x1800258d0;
const ERSC201_BUILD_LOBBY_KEY_VA: u64 = 0x1800ad6e0;
const ERSC201_SESSION_LOCK_VA: u64 = 0x1800f9828;

/// `OSM+0x58`, the session object every option action loads first. All five actions that share
/// the opening still start `mov rdi,[rcx+0x58]`.
const ERSC_NEXT_OBJECT_OFFSET: i64 = 0x58;
/// The value the guard field refuses to proceed past.
const ERSC_SESSION_GUARD_POISON: i32 = 0x7fff_ffff;

/// The session field group. The three move together and keep their relative spacing
/// (`guard = mutex+0x4c`, `state = mutex+0x50`), so a Seamless update that shifts the group is one
/// re-measurement with three consequences rather than three independent ones.
const ERSC201_SESSION_MUTEX_OFFSET: i64 = 0x100;
const ERSC201_SESSION_GUARD_OFFSET: i64 = 0x14c;
const ERSC201_SESSION_STATE_OFFSET: i64 = 0x150;

/// The session-state enum. Read exhaustively rather than inferred from the two actions: a scan of
/// every `mov dword [reg+STATE], imm32` in the build's real code enumerates the whole enum, which
/// is what a re-measurement has to redo -- Seamless has renumbered these before.
/// `scripts/ersc-disas.py states 0x150` reproduces it.
const ERSC201_STATE_IDLE: i32 = 0x01;
const ERSC201_STATE_SEARCHING: i32 = 0x0e;
const ERSC201_STATE_CANCELLING: i32 = 0x23;

const ERSC201_LOBBY_KEY_CTX_OFFSET: i64 = 0xc8;

/// The early-return block the inverted idle guard branches to, past the state write.
const ERSC201_INVADE_RETURN: u64 = 0x4e;

/// The option actions this repo pins, with every measured number in one place. `fatal_5` and
/// `fatal_6` are byte offsets from the function entry to the two error blocks the tail branches
/// to; both sit past the end of the pinned window, so they are named as offsets read off the
/// disassembly rather than as encoded displacements.
const ERSC201_INVADE: ErscAction = ErscAction {
    va: ERSC201_INVADE_ACTION_VA,
    lock_va: ERSC201_SESSION_LOCK_VA,
    mutex: ERSC201_SESSION_MUTEX_OFFSET,
    guard: ERSC201_SESSION_GUARD_OFFSET,
    state: ERSC201_SESSION_STATE_OFFSET,
    code: ERSC201_STATE_SEARCHING,
    // Eight bytes past where the shared tail would put them: exactly the relocated return block
    // above, which sits between the guard check and these two error blocks.
    fatal_5: 0x56,
    fatal_6: 0x60,
};
const ERSC201_CANCEL: ErscAction = ErscAction {
    va: ERSC201_CANCEL_ACTION_VA,
    lock_va: ERSC201_SESSION_LOCK_VA,
    mutex: ERSC201_SESSION_MUTEX_OFFSET,
    guard: ERSC201_SESSION_GUARD_OFFSET,
    state: ERSC201_SESSION_STATE_OFFSET,
    code: ERSC201_STATE_CANCELLING,
    fatal_5: 0x45,
    fatal_6: 0x4f,
};

/// The eight callee-saved pushes both `show` and `BuildLobbyKey` open with.
fn ersc_callee_saved_pushes(asm: &mut CodeAssembler) -> Result<(), iced_x86::IcedError> {
    asm.push(rbp)?;
    asm.push(r15)?;
    asm.push(r14)?;
    asm.push(r13)?;
    asm.push(r12)?;
    asm.push(rsi)?;
    asm.push(rdi)?;
    asm.push(rbx)
}

/// The shared opening of every ERSC option action: CET landing pad, two pushes, shadow space,
/// then the session load. Shared by five functions, which is exactly why no pin stops here.
fn ersc_option_action_opening(asm: &mut CodeAssembler) -> Result<(), iced_x86::IcedError> {
    asm.endbr64()?;
    asm.push(rsi)?;
    asm.push(rdi)?;
    asm.sub(rsp, 0x28)?;
    mov_r64_mem(asm, Register::RDI, Register::RCX, ERSC_NEXT_OBJECT_OFFSET)
}

/// Everything one option action needs said about it, so the bodies below differ only in their
/// measured numbers rather than in their code.
///
/// The three branch targets are given as OFFSETS from the function entry because that is how they
/// were read off the disassembly, and because two of them sit past the end of the pinned window --
/// naming the absolute address instead would mean typing an encoded displacement, which is the
/// hand-typed machine code this whole generator exists to avoid.
struct ErscAction {
    /// Function entry, so a branch target can be named as the address it actually is.
    va: u64,
    lock_va: u64,
    mutex: i64,
    guard: i64,
    state: i64,
    /// The value this action writes to the state field. What the action IS.
    code: i32,
    /// `mov ecx,5; call <fatal>` -- taken when the mutex is already held.
    fatal_5: u64,
    /// `mov [rdi+guard],0x7ffffffe; mov ecx,6; call <fatal>` -- taken when the guard is poisoned.
    fatal_6: u64,
}

/// The tail every option action shares: take the session mutex, bail if the guard is poisoned,
/// then write the action's code into the state field.
///
/// Nothing here is masked, so the two `jcc` displacements and the `call`'s rel32 are all pinned
/// exactly. They are constants of the build's own layout rather than relocations, and pinning
/// them is what turns "an option action is here" into "THIS option action is here".
fn ersc_option_action_tail(
    asm: &mut CodeAssembler,
    action: &ErscAction,
) -> Result<(), iced_x86::IcedError> {
    asm.lea(rsi, qword_ptr(rdi + action.mutex))?;
    asm.mov(rcx, rsi)?;
    asm.call(action.lock_va)?;
    asm.test(eax, eax)?;
    asm.jne(action.va + action.fatal_5)?;
    asm.cmp(dword_ptr(rdi + action.guard), ERSC_SESSION_GUARD_POISON)?;
    asm.je(action.va + action.fatal_6)?;
    asm.mov(dword_ptr(rdi + action.state), action.code)
}

/// A CANCEL action: no state precondition at all. It is the only unguarded shape here, which is
/// what made it the one entry point a masked BODY search could still find across an update.
fn ersc_cancel_action(
    asm: &mut CodeAssembler,
    action: &ErscAction,
) -> Result<(), iced_x86::IcedError> {
    ersc_option_action_opening(asm)?;
    ersc_option_action_tail(asm, action)
}

fn main() {
    prologue_build::declare_rerun(SUPPORT);

    generate(
        &[(
            PrologueSpec {
                name: "UPDATE_PROLOGUE",
                doc: "Opening bytes of `CS::FeSystemAnnounceView::Update`, so a game update that\n\
                      moves it fails closed instead of jumping mid-instruction. The check stops\n\
                      inside the `movaps` that spills `xmm6`, so this is the first\n\
                      `ANNOUNCE_UPDATE_CHECKED_BYTES` of `push rbx; sub rsp,0x30; movaps\n\
                      [rsp+0x20],xmm6`.",
                visibility: "pub",
                shape: Shape::Slice,
                image: Image::EldenRing,
                va: ANNOUNCE_UPDATE_VA,
                take: ANNOUNCE_UPDATE_CHECKED_BYTES,
                pin: &[0x40, 0x53, 0x48, 0x83, 0xec, 0x30, 0x0f, 0x29],
            },
            (|asm| {
                rex_push(asm, rbx)?;
                asm.sub(rsp, 0x30)?;
                asm.movaps(xmmword_ptr(rsp + 0x20), xmm6)?;
                Ok(())
            }) as prologue_build::Assemble,
        )],
        "generated_announce_prologues.rs",
    );

    generate(
        &[(
            PrologueSpec {
                name: "PLACE_NAME_LOOKUP_PROLOGUE",
                doc: "Opening bytes of the `PlaceName` getter, so a game update that moves it\n\
                      fails closed instead of calling into the middle of something else:\n\
                      `mov [rsp+8],rbx; push rdi; sub rsp,0x20`.",
                visibility: "pub",
                shape: Shape::Slice,
                image: Image::EldenRing,
                va: PLACE_NAME_LOOKUP_VA,
                take: 0,
                pin: &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20],
            },
            (|asm| {
                asm.mov(qword_ptr(rsp + 8), rbx)?;
                asm.push(rdi)?;
                asm.sub(rsp, 0x20)?;
                Ok(())
            }) as prologue_build::Assemble,
        )],
        "generated_place_name_prologues.rs",
    );

    generate(
        &[
            (
                PrologueSpec {
                    name: "V201_SHOW_PROLOGUE",
                    doc: "`show`, the option-menu builder, at `0x241a0`. The pin stops at the\n\
                          frame setup and is never used to identify anything: this is the one\n\
                          function the module HOOKS, so its bytes stop being the shipped bytes\n\
                          once the detour is in.",
                    visibility: "pub",
                    shape: Shape::Slice,
                    image: Image::Ersc201,
                    va: ERSC201_SHOW_VA,
                    take: 0,
                    pin: &[
                        0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
                        0x48, 0x81, 0xec, 0x88, 0x01, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    ersc_callee_saved_pushes(asm)?;
                    asm.sub(rsp, 0x188)?;
                    Ok(())
                }) as prologue_build::Assemble,
            ),
            (
                PrologueSpec {
                    name: "V201_INVADE_PROLOGUE",
                    doc: "\"Invade world\" at `0x25850`, through `mov [rdi+0x150],0xe` -- the\n\
                          ONE site that writes that value into that field, which is what makes\n\
                          this pin an identification rather than a shape match. Unlike `show` it\n\
                          is called and never hooked, so its bytes stay the shipped bytes for the\n\
                          life of the process.\n\n\
                          Method note for the next re-pin: the previous Seamless update INVERTED\n\
                          this idle guard, and `je`/`jne` differ in the OPCODE byte, which a\n\
                          masked search keeps. That single byte is why locating this function by\n\
                          body signature reported no match at any length while the cancel action\n\
                          beside it mapped cleanly. A masked body search can miss a function that\n\
                          did not even move.",
                    visibility: "pub",
                    shape: Shape::Slice,
                    image: Image::Ersc201,
                    va: ERSC201_INVADE_ACTION_VA,
                    take: 0,
                    pin: &[
                        0xf3, 0x0f, 0x1e, 0xfa, 0x56, 0x57, 0x48, 0x83, 0xec, 0x28, 0x48, 0x8b,
                        0x79, 0x58, 0x83, 0xbf, 0x50, 0x01, 0x00, 0x00, 0x01, 0x75, 0x37, 0x48,
                        0x8d, 0xb7, 0x00, 0x01, 0x00, 0x00, 0x48, 0x89, 0xf1, 0xe8, 0xb2, 0x3f,
                        0x0d, 0x00, 0x85, 0xc0, 0x75, 0x2c, 0x81, 0xbf, 0x4c, 0x01, 0x00, 0x00,
                        0xff, 0xff, 0xff, 0x7f, 0x74, 0x2a, 0xc7, 0x87, 0x50, 0x01, 0x00, 0x00,
                        0x0e, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| {
                    ersc_option_action_opening(asm)?;
                    asm.cmp(
                        dword_ptr(rdi + ERSC201_SESSION_STATE_OFFSET),
                        ERSC201_STATE_IDLE,
                    )?;
                    asm.jne(ERSC201_INVADE_ACTION_VA + ERSC201_INVADE_RETURN)?;
                    ersc_option_action_tail(asm, &ERSC201_INVADE)
                }) as prologue_build::Assemble,
            ),
            (
                PrologueSpec {
                    name: "V201_CANCEL_PROLOGUE",
                    doc: "\"Cancel search\" at `0x258d0`, through `mov [rdi+0x150],0x23`. The\n\
                          unguarded option-action shape: no state precondition, straight into the\n\
                          shared mutex/guard tail.",
                    visibility: "pub",
                    shape: Shape::Slice,
                    image: Image::Ersc201,
                    va: ERSC201_CANCEL_ACTION_VA,
                    take: 0,
                    pin: &[
                        0xf3, 0x0f, 0x1e, 0xfa, 0x56, 0x57, 0x48, 0x83, 0xec, 0x28, 0x48, 0x8b,
                        0x79, 0x58, 0x48, 0x8d, 0xb7, 0x00, 0x01, 0x00, 0x00, 0x48, 0x89, 0xf1,
                        0xe8, 0x3b, 0x3f, 0x0d, 0x00, 0x85, 0xc0, 0x75, 0x24, 0x81, 0xbf, 0x4c,
                        0x01, 0x00, 0x00, 0xff, 0xff, 0xff, 0x7f, 0x74, 0x22, 0xc7, 0x87, 0x50,
                        0x01, 0x00, 0x00, 0x23, 0x00, 0x00, 0x00,
                    ],
                },
                (|asm| ersc_cancel_action(asm, &ERSC201_CANCEL)) as prologue_build::Assemble,
            ),
            (
                PrologueSpec {
                    name: "V201_BUILD_LOBBY_KEY_PROLOGUE",
                    doc: "`BuildLobbyKey` at `0xad6e0`: eight callee-saved pushes, a `0x108`\n\
                          frame, the out-param in `rsi`, and the ctx field at `+0xc8`.\n\n\
                          Method note for the next re-pin: at the previous update this pin\'s\n\
                          19-byte prologue matched exactly one address and it was the WRONG\n\
                          function -- a different routine that happened to frame the same size.\n\
                          A unique prologue hit is a code shape, not a function; read the\n\
                          candidate before pinning it.",
                    visibility: "pub",
                    shape: Shape::Slice,
                    image: Image::Ersc201,
                    va: ERSC201_BUILD_LOBBY_KEY_VA,
                    take: 0,
                    pin: &[
                        0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
                        0x48, 0x81, 0xec, 0x08, 0x01, 0x00, 0x00, 0x48, 0x8d, 0xac, 0x24, 0x80,
                        0x00, 0x00, 0x00, 0x0f, 0x29, 0x75, 0x70, 0x48, 0xc7, 0x45, 0x68, 0xfe,
                        0xff, 0xff, 0xff, 0x48, 0x89, 0xd6, 0x48, 0x83, 0xb9, 0xc8, 0x00, 0x00,
                        0x00, 0x00,
                    ],
                },
                (|asm| {
                    ersc_callee_saved_pushes(asm)?;
                    asm.sub(rsp, 0x108)?;
                    asm.lea(rbp, qword_ptr(rsp + 0x80))?;
                    asm.movaps(xmmword_ptr(rbp + 0x70), xmm6)?;
                    asm.mov(qword_ptr(rbp + 0x68), -2)?;
                    asm.mov(rsi, rdx)?;
                    asm.cmp(qword_ptr(rcx + ERSC201_LOBBY_KEY_CTX_OFFSET), 0)?;
                    Ok(())
                }) as prologue_build::Assemble,
            ),
        ],
        "generated_ersc_prologues.rs",
    );
}
