// === Software (INT3) breakpoint engine ===========================================
// A scriptable code-breakpoint system driven entirely in-process (no Cheat Engine /
// GUI needed): patch 0xCC at a target VA, catch EXCEPTION_BREAKPOINT in the VEH,
// log the full register/stack context, restore the byte, single-step over the
// original instruction (trap flag), then re-arm. This is the same mechanism CE's
// VEH debugger uses; software INT3 + VEH works under wine/Proton (esync/fsync),
// unlike hardware DR data breakpoints. RVAs to break on are read from
// er-effects-breakpoints.txt (one hex RVA per line) in the game dir.
pub(crate) const EXCEPTION_BREAKPOINT_CODE: u32 = 0x80000003;
/// Win64 CONTEXT GP-register + EFlags offsets (ABI-fixed). EFlags carries the trap flag.
pub(crate) const CONTEXT_EFLAGS_OFFSET: usize = 0x44;
pub(crate) const CONTEXT_RAX_OFFSET: usize = 0x78;
pub(crate) const CONTEXT_RCX_OFFSET: usize = 0x80;
pub(crate) const CONTEXT_RDX_OFFSET: usize = 0x88;
pub(crate) const CONTEXT_RSP_OFFSET: usize = 0x98;
pub(crate) const CONTEXT_R8_OFFSET: usize = 0xb8;
pub(crate) const CONTEXT_R9_OFFSET: usize = 0xc0;
/// Trap flag (EFlags bit 8): set to single-step the restored instruction, then clear.
pub(crate) const TRAP_FLAG_MASK: u32 = 0x100;
/// INT3 opcode; the byte we patch in to trigger EXCEPTION_BREAKPOINT.
pub(crate) const INT3_OPCODE: u8 = 0xcc;
/// One INT3 byte; the patch/restore size.
pub(crate) const INT3_PATCH_SIZE: usize = 1;
/// Initial value for the VirtualProtect old-protection out-param.
pub(crate) const PROTECT_OLD_INIT: u32 = 0;
/// Radix for parsing hex RVAs from er-effects-breakpoints.txt.
pub(crate) const RVA_HEX_RADIX: u32 = 16;
/// Conservative bound for code/data RVAs accepted by the breakpoint config normalizer.
pub(crate) const SW_BP_RVA_LIMIT: usize = 0x5000000;
/// INT3 is one byte; on #BP the trap RIP points just past it, so the breakpoint
/// address = RIP - 1.
pub(crate) const INT3_RIP_BACKUP: usize = 1;
/// Max simultaneous software breakpoints.
pub(crate) const SW_BP_MAX: usize = 8;
/// Empty breakpoint slot sentinel (no address armed).
pub(crate) const SW_BP_EMPTY: usize = 0;
/// "no original byte recorded" sentinel (a real byte is 0..=0xff, so 0x100 is free).
pub(crate) const SW_BP_ORIG_NONE: usize = 0x100;
/// Mask to recover the original byte from the stored slot value.
pub(crate) const SW_BP_ORIG_BYTE_MASK: usize = 0xff;
/// Per-breakpoint hit-log cap (so a per-frame breakpoint does not flood the log).
pub(crate) const SW_BP_MAX_LOGS_PER_BP: usize = 400;
/// Pending-rearm sentinel (no breakpoint awaiting re-arm on the next single-step).
pub(crate) const SW_BP_REARM_NONE: usize = 0;
pub(crate) const SW_BP_HIT_INCREMENT: usize = 1;
/// Initial per-breakpoint hit counter.
pub(crate) const SW_BP_HITS_INIT: usize = 0;
pub(crate) const SW_BP_SLOT_STEP: usize = 1;
/// Number of stack qwords to dump on a breakpoint hit (args spilled past r9 + locals).
pub(crate) const SW_BP_STACK_DUMP_QWORDS: usize = 40;
pub(crate) static SW_BP_ADDR: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_EMPTY) }; SW_BP_MAX];
pub(crate) static SW_BP_ORIG: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_ORIG_NONE) }; SW_BP_MAX];
pub(crate) static SW_BP_HITS: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_HITS_INIT) }; SW_BP_MAX];
/// Address awaiting re-arm on the next single-step (set in the #BP handler, consumed
/// in the single-step handler). Single global: our breakpoints fire on one menu thread.
pub(crate) static SW_BP_REARM_PENDING: AtomicUsize = AtomicUsize::new(SW_BP_REARM_NONE);
pub(crate) static SW_BP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Diagnostic: count #BP exceptions our VEH sees that are NOT at one of our armed addresses,
/// to distinguish "VEH gets #BP but addr mismatch" from "VEH never sees #BP" under wine.
pub(crate) static SW_BP_UNMATCHED_LOGGED: AtomicUsize = AtomicUsize::new(SW_BP_HITS_INIT);
pub(crate) const SW_BP_MAX_UNMATCHED_LOGS: usize = 8;

