//! Automated repair of BROKEN skin mods (see docs/SKIN-SAFETY-AND-REPAIR.md).
//!
//! A broken mod ships the champion's ROOT character record
//! (`data/characters/<champ>/<champ>.bin`) — the file that defines abilities and
//! breaks the game when injected. Repair strips ONLY that record and repacks,
//! keeping the cosmetic skin (mesh/texture/VFX + skin records). The game then
//! uses its own current ability data (abilities work) while the mod's look still
//! applies.
//!
//! Self-contained: uses the bundled cslol `wad-extract`/`wad-make`, and needs NO
//! hashtable — the offending chunk is deleted from the hash-named extraction by
//! its xxh64 path hash. The cslol extract→make round-trip is hash-preserving.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use xxhash_rust::xxh64::xxh64;

use crate::skins::champ_alias;
use crate::skins::injection::tools;
use crate::skins::paths;
use crate::skins::slog::{log_info, log_warn};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hide_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Repair a broken ability-override mod IN PLACE. The original is backed up to
/// `<file>.chudbak` first; on ANY failure the original file is left untouched.
/// Returns a short summary of what was stripped. Idempotent-ish: a mod that no
/// longer contains the record returns an error ("nothing to repair").
pub fn repair_ability_override(mod_path: &Path, champion_id: i64) -> Result<String, String> {
    let alias = champ_alias::champ_alias(champion_id)
        .ok_or_else(|| "unknown champion".to_string())?
        .to_lowercase();
    let ability_path = format!("data/characters/{alias}/{alias}.bin");
    let ability_hex = format!("{:016x}", xxh64(ability_path.as_bytes(), 0));

    let toolp = tools::detect_tools(&tools::cslol_tools_dir());
    if !toolp.wad_extract.exists() || !toolp.wad_make.exists() {
        return Err("cslol wad tools are not available".to_string());
    }

    let work = paths::injection_extract_cache_dir().join(format!(".repair_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("create work dir: {e}"))?;

    let result = (|| -> Result<String, String> {
        // 1. Pull the WAD out of the .fantome (preserve its entry name).
        let (wad_entry, wad_tmp) = extract_wad(mod_path, &work)?;

        // 2. Extract the WAD to chunk files.
        let ext = work.join("ext");
        run_tool(&toolp.wad_extract, &[wad_tmp.as_os_str(), ext.as_os_str()], "wad-extract")?;

        // 3. Delete the ability record (BOTH the resolved real path AND any
        //    hash-named form — wad-extract's output depends on the hashtable).
        let removed = remove_ability_chunk(&ext, &alias, &ability_hex);
        if removed == 0 {
            return Err("nothing to repair — the ability record isn't present".to_string());
        }

        // 4. Repack the remaining chunks (ABSOLUTE dst — a relative one fails).
        let repaired_wad = work.join("repaired.wad.client");
        run_tool(&toolp.wad_make, &[ext.as_os_str(), repaired_wad.as_os_str()], "wad-make")?;

        // 5. Rebuild the .fantome (original entries, WAD swapped) into a temp.
        let repaired_fantome = work.join("repaired.fantome");
        rebuild_fantome(mod_path, &wad_entry, &repaired_wad, &repaired_fantome)?;

        // 6. Back up the original, then swap the repaired file into place.
        let backup = mod_path.with_extension("chudbak");
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(mod_path, &backup).map_err(|e| format!("back up original: {e}"))?;
        if let Err(e) = std::fs::copy(&repaired_fantome, mod_path) {
            let _ = std::fs::rename(&backup, mod_path);
            return Err(format!("write repaired mod: {e}"));
        }
        log_info!("[REPAIR] Repaired '{}' — stripped {ability_path} ({removed} chunk); backup at {}", mod_path.display(), backup.display());
        Ok(format!("Removed the ability data ({ability_path}); the skin's visuals were kept."))
    })();

    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Copy the `.wad.client` entry out of a `.fantome` zip to `work/original.wad.client`.
/// Returns its in-archive entry name (to preserve on repack) and the temp path.
fn extract_wad(fantome: &Path, work: &Path) -> Result<(String, PathBuf), String> {
    let file = std::fs::File::open(fantome).map_err(|e| format!("open mod: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("read mod archive: {e}"))?;
    let entry_name = (0..zip.len()).find_map(|i| {
        let e = zip.by_index(i).ok()?;
        let n = e.name().to_string();
        n.to_lowercase().replace('\\', "/").ends_with(".wad.client").then_some(n)
    });
    let entry_name = entry_name.ok_or_else(|| "mod has no WAD".to_string())?;
    let dst = work.join("original.wad.client");
    let mut src = zip.by_name(&entry_name).map_err(|e| format!("read WAD entry: {e}"))?;
    let mut out = std::fs::File::create(&dst).map_err(|e| format!("write WAD temp: {e}"))?;
    std::io::copy(&mut src, &mut out).map_err(|e| format!("copy WAD: {e}"))?;
    Ok((entry_name, dst))
}

/// Delete the champion root character record from the extracted tree, in BOTH
/// forms wad-extract can produce: the resolved REAL path
/// `data/characters/<alias>/<alias>.bin` (when a hashtable is present), and any
/// top-level hash-named file `[0x]<hex>[.ext]` (when it isn't). Returns the
/// count removed.
fn remove_ability_chunk(ext_dir: &Path, alias: &str, hex: &str) -> usize {
    let mut removed = 0;

    // Resolved real path.
    let real = ext_dir.join("data").join("characters").join(alias).join(format!("{alias}.bin"));
    if real.is_file() && std::fs::remove_file(&real).is_ok() {
        removed += 1;
    }

    // Hash-named fallback (top level of the extraction).
    if let Ok(entries) = std::fs::read_dir(ext_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let stem = name.strip_prefix("0x").unwrap_or(name).split('.').next().unwrap_or("");
            if stem.eq_ignore_ascii_case(hex) && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Rebuild a `.fantome`: copy every original entry EXCEPT the WAD, then write the
/// repaired WAD back under its original entry name.
fn rebuild_fantome(original: &Path, wad_entry: &str, repaired_wad: &Path, dst: &Path) -> Result<(), String> {
    let src_file = std::fs::File::open(original).map_err(|e| format!("open original: {e}"))?;
    let mut src = zip::ZipArchive::new(src_file).map_err(|e| format!("read original: {e}"))?;
    let out_file = std::fs::File::create(dst).map_err(|e| format!("create repaired: {e}"))?;
    let mut out = zip::ZipWriter::new(out_file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    for i in 0..src.len() {
        let mut entry = src.by_index(i).map_err(|e| format!("read entry {i}: {e}"))?;
        let name = entry.name().to_string();
        if name == wad_entry {
            continue; // replaced below
        }
        if entry.is_dir() {
            continue;
        }
        out.start_file(&name, opts).map_err(|e| format!("write entry {name}: {e}"))?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).map_err(|e| format!("read entry {name}: {e}"))?;
        out.write_all(&buf).map_err(|e| format!("copy entry {name}: {e}"))?;
    }
    // The repaired WAD under its original entry name.
    out.start_file(wad_entry, opts).map_err(|e| format!("write WAD entry: {e}"))?;
    let wad_bytes = std::fs::read(repaired_wad).map_err(|e| format!("read repaired WAD: {e}"))?;
    out.write_all(&wad_bytes).map_err(|e| format!("copy repaired WAD: {e}"))?;
    out.finish().map_err(|e| format!("finalize repaired: {e}"))?;
    Ok(())
}

/// Run a cslol tool with hidden console, mapping a non-zero exit / spawn failure
/// to an Err with the tool's stderr tail.
fn run_tool(exe: &Path, args: &[&std::ffi::OsStr], label: &str) -> Result<(), String> {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    let output = hide_window(&mut cmd).output().map_err(|e| format!("{label} spawn: {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let tail: String = err.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        log_warn!("[REPAIR] {label} failed: {tail}");
        return Err(format!("{label} failed: {tail}"));
    }
    Ok(())
}
