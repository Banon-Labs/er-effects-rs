//! Generates the `CS::SoftwareKeyboard` prologues this crate byte-checks before it calls them.
//!
//! Moved here from `er-quickload`'s build script with the keyboard machinery itself
//! (`software_keyboard.rs`): the recipe is driven by two surfaces -- the save picker's path editor
//! and the System>Quit link field -- and the second has to work with no product DLL behind it, so
//! the generated table has to live on this side of the crate boundary. Generating it in both build
//! scripts would write each address out twice, which is exactly what
//! `scripts/check-rva-alias-drift.py` refuses.
//!
//! See `build-support/prologue_build.rs` for why these are generated from named instructions rather
//! than hand-typed, and for what verifies the result against the game image.

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
    Assemble, Image, PrologueSpec, Shape, cmp_r32_rm32, generate, mov_r64_mem_base, mov_r64_rm64,
    rex_push,
};

const SUPPORT: &str = "../../build-support/prologue_build.rs";

/// The SoftwareKeyboard recipe the save-picker path editor drives.
const SOFTWARE_KEYBOARD_JOB_CTOR_VA: u64 = 0x14081be30;
const SOFTWARE_KEYBOARD_RESULT_GATE_VA: u64 = 0x14081d3d0;
const SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_VA: u64 = 0x14081d220;
const SOFTWARE_KEYBOARD_VALIDATOR_INIT_VA: u64 = 0x140e70920;
const SOFTWARE_KEYBOARD_VALIDATOR_DTOR_VA: u64 = 0x140e70960;
const SOFTWARE_KEYBOARD_ENTER_NAME_VA: u64 = 0x140e70c00;
const SOFTWARE_KEYBOARD_SET_INITIAL_VA: u64 = 0x140e709f0;
const SOFTWARE_KEYBOARD_SET_MAX_VA: u64 = 0x142416ee0;
const GAME_HEAP_ALLOC_VA: u64 = 0x141eb9ed0;
/// The EnterName preset's window stops inside its `sub rsp,0x70`, so the assembled sequence is
/// one byte longer than the constant.
const SOFTWARE_KEYBOARD_ENTER_NAME_CHECKED_BYTES: usize = 7;

/// The `mov [rsp+8],rcx; push rbx; sub rsp,0x30` opening shared by the validator's init and
/// dtor -- byte-identical, which is why both are checked at their own RVA.
fn validator_prologue(asm: &mut CodeAssembler) -> Result<(), iced_x86::IcedError> {
    asm.mov(qword_ptr(rsp + 8), rcx)?;
    asm.push(rbx)?;
    asm.sub(rsp, 0x30)
}

const VALIDATOR_PIN: &[u8] = &[0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x48, 0x83, 0xec, 0x30];

fn main() {
    prologue_build::declare_rerun(SUPPORT);

    generate(
        &[
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_JOB_CTOR_SIG",
                    doc: "`mov [rsp+8],rcx; push rbx/rbp/rsi/rdi/r14; sub rsp,0x30`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_JOB_CTOR_VA,
                    take: 0,
                    pin: &[
                        0x48, 0x89, 0x4c, 0x24, 0x08, 0x53, 0x55, 0x56, 0x57, 0x41, 0x56, 0x48,
                        0x83, 0xec, 0x30,
                    ],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 8), rcx)?;
                    asm.push(rbx)?;
                    asm.push(rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.push(r14)?;
                    asm.sub(rsp, 0x30)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_RESULT_GATE_SIG",
                    doc: "`mov [rsp+0x18],r8; push rbp/rsi/rdi; sub rsp,0x40`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_RESULT_GATE_VA,
                    take: 0,
                    pin: &[
                        0x4c, 0x89, 0x44, 0x24, 0x18, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec, 0x40,
                    ],
                },
                (|asm| {
                    asm.mov(qword_ptr(rsp + 0x18), r8)?;
                    asm.push(rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x40)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_SIG",
                    doc: "Seven callee-saved pushes: `rbp, rsi, rdi, r12, r13, r14, r15`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_TERMINAL_CALLBACK_VA,
                    take: 0,
                    pin: &[
                        0x40, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
                    ],
                },
                (|asm| {
                    rex_push(asm, rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.push(r12)?;
                    asm.push(r13)?;
                    asm.push(r14)?;
                    asm.push(r15)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_VALIDATOR_INIT_SIG",
                    doc: "`mov [rsp+8],rcx; push rbx; sub rsp,0x30`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_VALIDATOR_INIT_VA,
                    take: 0,
                    pin: VALIDATOR_PIN,
                },
                validator_prologue as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_VALIDATOR_DTOR_SIG",
                    doc: "Byte-identical to the validator's init prologue, which is why each is\n\
                          checked at its own RVA rather than one standing in for the other.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_VALIDATOR_DTOR_VA,
                    take: 0,
                    pin: VALIDATOR_PIN,
                },
                validator_prologue as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_ENTER_NAME_SIG",
                    doc: "`push rbp/rsi/rdi; sub rsp,0x70`. The window stops inside that `sub`,\n\
                          so the assembled sequence is one byte longer than the constant.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_ENTER_NAME_VA,
                    take: SOFTWARE_KEYBOARD_ENTER_NAME_CHECKED_BYTES,
                    pin: &[0x40, 0x55, 0x56, 0x57, 0x48, 0x83, 0xec],
                },
                (|asm| {
                    rex_push(asm, rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.sub(rsp, 0x70)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_SET_INITIAL_SIG",
                    doc: "`push rbp/rsi/rdi; lea rbp,[rsp-0x47]`.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_SET_INITIAL_VA,
                    take: 0,
                    pin: &[0x40, 0x55, 0x56, 0x57, 0x48, 0x8d, 0x6c, 0x24, 0xb9],
                },
                (|asm| {
                    rex_push(asm, rbp)?;
                    asm.push(rsi)?;
                    asm.push(rdi)?;
                    asm.lea(rbp, qword_ptr(rsp - 0x47))?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "SOFTWARE_KEYBOARD_SET_MAX_SIG",
                    doc: "`mov eax,1; cmp edx,eax; cmovge eax,edx` -- the clamp that makes the\n\
                          max-length setter's floor 1.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: SOFTWARE_KEYBOARD_SET_MAX_VA,
                    take: 0,
                    pin: &[0xb8, 0x01, 0x00, 0x00, 0x00, 0x3b, 0xd0, 0x0f, 0x4d, 0xc2],
                },
                (|asm| {
                    asm.mov(eax, 1)?;
                    cmp_r32_rm32(asm, Register::EDX, Register::EAX)?;
                    asm.cmovge(eax, edx)?;
                    Ok(())
                }) as Assemble,
            ),
            (
                PrologueSpec {
                    name: "GAME_HEAP_ALLOC_SIG",
                    doc: "The allocator thunk: `mov rax,[r8]; mov r9,r8; mov r8,rdx` before it\n\
                          tail-jumps through the allocator vtable.",
                    visibility: "",
                    shape: Shape::Slice,
                    image: Image::EldenRing,
                    va: GAME_HEAP_ALLOC_VA,
                    take: 0,
                    pin: &[0x49, 0x8b, 0x00, 0x4d, 0x8b, 0xc8, 0x4c, 0x8b, 0xc2],
                },
                (|asm| {
                    mov_r64_mem_base(asm, Register::RAX, Register::R8)?;
                    mov_r64_rm64(asm, Register::R9, Register::R8)?;
                    mov_r64_rm64(asm, Register::R8, Register::RDX)?;
                    Ok(())
                }) as Assemble,
            ),
        ],
        "generated_software_keyboard_prologues.rs",
    );
}
