//! Generates the prologue this DLL byte-checks, from named `iced-x86` instructions.
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
use prologue_build::{Assemble, Image, PrologueSpec, Shape, generate, mov_r32_mem, rex_push};

const SUPPORT: &str = "../../build-support/prologue_build.rs";

/// The lock-on point-owner resolver: `ChrIns *FUN_140713db0(ActPnt *point)`.
const LOCK_ON_POINT_OWNER_VA: u64 = 0x140713db0;

fn main() {
    prologue_build::declare_rerun(SUPPORT);
    generate(
        &[(
            PrologueSpec {
                name: "LOCK_ON_POINT_OWNER_PROLOGUE",
                doc: "`push rbx; sub rsp,0x20; mov eax,[rcx+0x78]; lea rbx,[rcx+0x78]` -- the\n\
                      opening of the lock-on point-owner resolver, up to and including the load\n\
                      of the point's `FieldInsHandle`.\n\
                      \n\
                      Two encodings are named rather than left to the assembler. The `push` is\n\
                      the redundant-prefix form the compiler emitted (`40 53`), and the `mov` is\n\
                      the `8b` rm32 direction; picking either the other way is a one-byte\n\
                      difference that disarms the gate on every launch.\n\
                      \n\
                      Identical in 1.16.2 and 1.17: the same 13 bytes sit at 0x140713db0 and at\n\
                      0x140714c00, which is why one pin covers both builds.",
                visibility: "",
                shape: Shape::Array,
                image: Image::EldenRing,
                va: LOCK_ON_POINT_OWNER_VA,
                take: 0,
                pin: &[
                    0x40, 0x53, 0x48, 0x83, 0xec, 0x20, 0x8b, 0x41, 0x78, 0x48, 0x8d, 0x59, 0x78,
                ],
            },
            (|asm| {
                rex_push(asm, rbx)?;
                asm.sub(rsp, 0x20)?;
                mov_r32_mem(asm, Register::EAX, Register::RCX, 0x78)?;
                asm.lea(rbx, qword_ptr(rcx + 0x78))?;
                Ok(())
            }) as Assemble,
        )],
        "generated_prologues.rs",
    );
}
