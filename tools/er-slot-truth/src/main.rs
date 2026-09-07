//! Print which character slots a save container actually holds -- the same question, and the same
//! parser, the DLL now asks before it will wait on a Continue slot.
//!
//! Why this exists. On 2026-09-03 `er-quickload.toml` named `slot=1`, Seamless was loaded so the
//! game read `ER0000.co2`, and that container holds exactly one character, in slot 0. Slot 1 could
//! never fill, but the boot could not tell "vacant" from "still filling" and spent 1800 ticks of
//! patience finding out; the run reached the intro cutscene first. The DLL now asks the container
//! before waiting (`configured_slot_holds_a_character`), and this tool asks the identical question
//! offline through the identical `er_save_loader::bnd4::active_character_slots`, so the runtime
//! decision can be predicted -- and reviewed -- without launching the game.
//!
//! Usage: cargo run -p er-slot-truth -- <save container>... [--slot N]

fn main() {
    let mut paths: Vec<String> = Vec::new();
    let mut want: Vec<usize> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--slot" {
            match args.next().and_then(|v| v.parse().ok()) {
                Some(slot) => want.push(slot),
                None => {
                    eprintln!("--slot needs a number");
                    std::process::exit(2);
                }
            }
        } else {
            paths.push(arg);
        }
    }
    if paths.is_empty() {
        eprintln!("usage: er-slot-truth <save container>... [--slot N]");
        std::process::exit(2);
    }
    if want.is_empty() {
        want = vec![0, 1];
    }
    for path in paths {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                println!("{path}\n   unreadable: {err}");
                continue;
            }
        };
        println!("{path}");
        match er_save_loader::bnd4::active_character_slots(&bytes) {
            Ok(slots) => {
                let occupied: Vec<usize> = slots.iter().map(|s| s.slot).collect();
                println!("   occupied slots: {occupied:?}");
                for s in &slots {
                    println!("     slot {} = {:?} (level {})", s.slot, s.name, s.level);
                }
                for slot in &want {
                    println!(
                        "   slot {slot} holds a character: {}",
                        slots.iter().any(|s| s.slot == *slot)
                    );
                }
            }
            // A refusal is not "no characters": it means the question was not answered, which is
            // exactly the `None` the DLL treats as "keep waiting" rather than "reject the slot".
            Err(err) => println!("   parse refused (DLL would keep waiting): {err:?}"),
        }
    }
}
