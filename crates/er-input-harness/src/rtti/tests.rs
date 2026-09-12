//! A fake address space laid out the way the running game's is, so the job-graph resolution can be
//! pinned without a game.
//!
//! The regression these exist for is a specific qword: `0x2f003a00320063`, which
//! `currentTopMenuJob+0x130` read on 1.17.1 (bd er-effects-rs-h09b). It is UTF-16 text, it is
//! larger than the old `HEAP_LO` filter, and the old resolution handed it to every caller as a
//! `CS::MenuWindow`. `refuses_a_utf16_asset_path_as_a_window` is that exact value, in that exact
//! field, on a graph shaped like the live one.

use std::collections::BTreeMap;

use super::{GameMemory, class_name, derives_from, is_class};
use crate::game_mem::resolve_top_window;

/// The game's image base, the same value the live walk is handed.
const BASE: usize = 0x1_4000_0000;

/// A sparse byte map standing in for the process's address space: a read of an address nobody wrote
/// answers `None`, exactly as `ReadProcessMemory` does on an unmapped page.
#[derive(Default)]
struct FakeMemory {
    bytes: BTreeMap<usize, u8>,
}

impl FakeMemory {
    fn u8(&mut self, addr: usize, value: u8) {
        self.bytes.insert(addr, value);
    }
    fn u32(&mut self, addr: usize, value: u32) {
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.u8(addr + index, byte);
        }
    }
    fn word(&mut self, addr: usize, value: usize) {
        for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
            self.u8(addr + index, byte);
        }
    }
    fn cstr(&mut self, addr: usize, value: &str) {
        for (index, byte) in value.bytes().enumerate() {
            self.u8(addr + index, byte);
        }
        self.u8(addr + value.len(), 0);
    }
}

impl GameMemory for FakeMemory {
    fn read_usize(&self, addr: usize) -> Option<usize> {
        let mut out = [0u8; core::mem::size_of::<usize>()];
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = *self.bytes.get(&(addr + index))?;
        }
        Some(usize::from_le_bytes(out))
    }
    fn read_u32(&self, addr: usize) -> Option<u32> {
        let mut out = [0u8; 4];
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = *self.bytes.get(&(addr + index))?;
        }
        Some(u32::from_le_bytes(out))
    }
    fn read_bytes(&self, addr: usize, out: &mut [u8]) -> usize {
        let mut read = 0;
        for (index, slot) in out.iter_mut().enumerate() {
            match self.bytes.get(&(addr + index)) {
                Some(byte) => {
                    *slot = *byte;
                    read = index + 1;
                }
                None => break,
            }
        }
        read
    }
}

/// Hands out non-overlapping chunks of the fake image's `.rdata`, so a test never has to pick
/// addresses by hand and two classes can never land on each other.
struct Rdata {
    next: usize,
}

impl Rdata {
    fn take(&mut self, size: usize) -> usize {
        let at = self.next;
        self.next += size.next_multiple_of(0x10);
        at
    }
}

/// Lay down one class's `TypeDescriptor`, `ClassHierarchyDescriptor`, `CompleteObjectLocator` and
/// vtable, and return the vtable address -- which is what an object of that class stores at `+0`.
///
/// `bases` names the class's own mangled name first, the way MSVC's base-class array does, followed
/// by whatever it derives from.
fn define_class(mem: &mut FakeMemory, rdata: &mut Rdata, bases: &[&str]) -> usize {
    let mut base_descriptors = Vec::new();
    for name in bases {
        let type_descriptor = rdata.take(0x20 + name.len());
        mem.word(BASE + type_descriptor, 0);
        mem.cstr(BASE + type_descriptor + 0x10, name);
        let descriptor = rdata.take(0x20);
        mem.u32(BASE + descriptor, type_descriptor as u32);
        base_descriptors.push((type_descriptor, descriptor));
    }
    let array = rdata.take(4 * base_descriptors.len().max(1));
    for (index, (_, descriptor)) in base_descriptors.iter().enumerate() {
        mem.u32(BASE + array + index * 4, *descriptor as u32);
    }
    let chd = rdata.take(0x10);
    mem.u32(BASE + chd, 0);
    mem.u32(BASE + chd + 4, 0);
    mem.u32(BASE + chd + 8, base_descriptors.len() as u32);
    mem.u32(BASE + chd + 0xc, array as u32);

    let col = rdata.take(0x20);
    mem.u32(BASE + col, 1); // x64 signature
    mem.u32(BASE + col + 4, 0);
    mem.u32(BASE + col + 8, 0);
    mem.u32(BASE + col + 0xc, base_descriptors[0].0 as u32);
    mem.u32(BASE + col + 0x10, chd as u32);
    mem.u32(BASE + col + 0x14, col as u32); // the self-RVA that makes this provable

    let vtable_block = rdata.take(0x40);
    let vtable = BASE + vtable_block + 8;
    mem.word(vtable - 8, BASE + col);
    mem.word(vtable, 0xdead_0000);
    vtable
}

const MENU_WINDOW_JOB: &str = ".?AVMenuWindowJob@CS@@";
const MENU_WINDOW: &str = ".?AVMenuWindow@CS@@";
const OPTION_SETTING_DIALOG: &str = ".?AVOptionSettingTopDialog@CS@@";
const FINALIZE_CALLBACK_JOB: &str = ".?AVFinalizeCallbackJob@CS@@";
const FIX_ORDER_JOB_SEQUENCE: &str = ".?AVFixOrderJobSequence@CS@@";
const WAIT_FRAME_JOB: &str = ".?AVWaitFrameJob@CS@@";

/// `CS::MenuWindowJob::owningMenuWindow`, the field the walk reads once it knows the class.
const OWNING_MENU_WINDOW_OFFSET: usize = 0x130;
/// `CS::FinalizeCallbackJob`'s inner job, and `CS::FixOrderJobSequence`'s job vector -- the two
/// links `CSPopupMenu::StartTopMenuJob` actually builds.
const FINALIZE_INNER_JOB_OFFSET: usize = 0x10;
const SEQUENCE_JOBS_OFFSET: usize = 0x18;

/// The live shape: `currentTopMenuJob` -> `FinalizeCallbackJob` -> `FixOrderJobSequence` ->
/// [wait-frame job, `MenuWindowJob`] -> the pane's window.
struct Graph {
    mem: FakeMemory,
    root: usize,
    window: usize,
    menu_window_job: usize,
}

fn build_graph() -> Graph {
    let mut mem = FakeMemory::default();
    let mut rdata = Rdata { next: 0x0100_0000 };

    let job_vtable = define_class(&mut mem, &mut rdata, &[MENU_WINDOW_JOB]);
    let window_vtable = define_class(&mut mem, &mut rdata, &[OPTION_SETTING_DIALOG, MENU_WINDOW]);
    let finalize_vtable = define_class(&mut mem, &mut rdata, &[FINALIZE_CALLBACK_JOB]);
    let sequence_vtable = define_class(&mut mem, &mut rdata, &[FIX_ORDER_JOB_SEQUENCE]);
    let wait_vtable = define_class(&mut mem, &mut rdata, &[WAIT_FRAME_JOB]);

    let window = 0x2a00_0000;
    mem.word(window, window_vtable);

    let menu_window_job = 0x2b00_0000;
    mem.word(menu_window_job, job_vtable);
    mem.word(menu_window_job + OWNING_MENU_WINDOW_OFFSET, window);

    let wait_job = 0x2c00_0000;
    mem.word(wait_job, wait_vtable);

    let sequence = 0x2d00_0000;
    mem.word(sequence, sequence_vtable);
    mem.word(sequence + SEQUENCE_JOBS_OFFSET, wait_job);
    mem.word(sequence + SEQUENCE_JOBS_OFFSET + 8, menu_window_job);

    let root = 0x2e00_0000;
    mem.word(root, finalize_vtable);
    mem.word(root + FINALIZE_INNER_JOB_OFFSET, sequence);

    Graph {
        mem,
        root,
        window,
        menu_window_job,
    }
}

#[test]
fn resolves_the_window_through_the_wrapper_jobs() {
    let graph = build_graph();
    let found = resolve_top_window(&graph.mem, BASE, graph.root);
    assert_eq!(found.window, graph.window);
    assert_eq!(found.owners, 1);
}

#[test]
fn names_the_classes_it_walks_through() {
    let graph = build_graph();
    assert_eq!(
        class_name(&graph.mem, BASE, graph.root).map(|c| c.as_str().to_string()),
        Some(FINALIZE_CALLBACK_JOB.to_string())
    );
    assert!(is_class(
        &graph.mem,
        BASE,
        graph.menu_window_job,
        MENU_WINDOW_JOB
    ));
}

#[test]
fn a_window_subclass_still_counts_as_a_menu_window() {
    let graph = build_graph();
    // The live window is never a `CS::MenuWindow` itself -- 107 classes derive from it -- so an
    // exact-class check would reject every real pane.
    assert!(!is_class(&graph.mem, BASE, graph.window, MENU_WINDOW));
    assert!(derives_from(&graph.mem, BASE, graph.window, MENU_WINDOW));
    assert!(!derives_from(
        &graph.mem,
        BASE,
        graph.window,
        MENU_WINDOW_JOB
    ));
}

#[test]
fn refuses_a_utf16_asset_path_as_a_window() {
    // The 1.17.1 defect, reproduced: the top job is not a `MenuWindowJob`, and the qword at its
    // `+0x130` is the UTF-16 for "c2:/aet/aet050/A..." sitting in the heap behind the object.
    let mut graph = build_graph();
    graph.mem.word(
        graph.root + OWNING_MENU_WINDOW_OFFSET,
        0x002f_003a_0032_0063,
    );
    // Cut the graph so nothing else can answer, leaving only the bad qword to be tempted by.
    graph.mem.word(graph.root + FINALIZE_INNER_JOB_OFFSET, 0);
    let found = resolve_top_window(&graph.mem, BASE, graph.root);
    assert_eq!(found.window, 0, "a UTF-16 asset path is not a MenuWindow");
    assert_eq!(found.owners, 0);
}

#[test]
fn refuses_a_menu_window_job_whose_window_is_not_one() {
    let mut graph = build_graph();
    // A plausible heap pointer that is not a polymorphic object: the shape a stale or reallocated
    // window would have. The job class is right and the window still has to prove itself.
    graph.mem.word(
        graph.menu_window_job + OWNING_MENU_WINDOW_OFFSET,
        0x3f00_0000,
    );
    let found = resolve_top_window(&graph.mem, BASE, graph.root);
    assert_eq!(found.window, 0);
}

#[test]
fn refuses_a_locator_that_does_not_point_at_itself() {
    let mut graph = build_graph();
    let vtable = graph.mem.read_usize(graph.window).expect("window vtable");
    let col = graph.mem.read_usize(vtable - 8).expect("locator");
    // Break only the self-RVA. Everything else about the object still looks right, which is the
    // point: this one field is what makes a locator provable rather than plausible.
    graph.mem.u32(col + 0x14, 0xdead_beef);
    assert!(class_name(&graph.mem, BASE, graph.window).is_none());
    assert!(!derives_from(&graph.mem, BASE, graph.window, MENU_WINDOW));
}
