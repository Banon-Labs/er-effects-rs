//! Report, and optionally repair, the SteamID64 an Elden Ring save container is bound to.
//!
//! The game refuses a container whose embedded id is not the account loading it, and says so as
//! "Failed to load save data. Select OK to try again." -- which names neither the id nor the slot,
//! so the only way to tell that refusal apart from a corrupt container is to read the ids out.
//!
//! The product already rebinds ids, but only on the save-redirect/staging branch
//! (`save_redirect::path_hooks`). A `save-override: DEFAULT-USER-SAVE` boot takes no redirect, so
//! nothing rebinds and a foreign-id container reaches the game unpatched. This example is that same
//! `er_save_loader::bnd4` code reachable from the host, for a container already sitting in APPDATA.
//!
//! Read-only unless `--write <steamid>` is given; `--write` rewrites in place and re-MD5s every
//! entry it touches, because a patched body with a stale entry hash is refused just as firmly.
//!
//! ```text
//! cargo run -p er-save-loader --example steam_id_report -- <ER0000.sl2> [--write <steamid64>]
//! ```

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: steam_id_report <save> [--write <steamid64>]")?;
    let mut write_to: Option<u64> = None;
    while let Some(flag) = args.next() {
        if flag == "--write" {
            write_to = Some(args.next().ok_or("--write needs a SteamID64")?.parse()?);
        } else {
            return Err(format!("unknown argument {flag}").into());
        }
    }

    let mut bytes = std::fs::read(&path)?;
    println!("{path} ({} bytes)", bytes.len());
    let locations =
        er_save_loader::bnd4::steam_id_locations(&bytes).map_err(|e| format!("{e:?}"))?;
    for location in locations {
        println!(
            "  {} body+0x{:x} file+0x{:x} = {}",
            location.entry_name, location.body_offset, location.file_offset, location.value
        );
    }

    let Some(target) = write_to else {
        println!("read-only: pass --write <steamid64> to rebind");
        return Ok(());
    };
    let report = er_save_loader::bnd4::normalize_steam_id_in_place(&mut bytes, target)
        .map_err(|e| format!("{e:?}"))?;
    std::fs::write(&path, &bytes)?;
    println!(
        "rebound to {target}: slots_seen={} slots_patched={} md5_rewritten={} user_data10_patched={}",
        report.character_slots_seen,
        report.character_slots_patched,
        report.md5_rewritten,
        report.user_data10_patched
    );
    Ok(())
}
