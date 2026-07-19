//! "Skin name on the loading screen."
//!
//! The loading-screen name is NOT baked into the loadscreen splash `.tex` (that
//! file is pure art) — the game draws it as a separate label whose text comes
//! from the active **SkinID's** display name in the localized string table
//! (`data/menu/<locale>/lol.stringtable`, an RST v5 blob in
//! `Localized/Global.<locale>.wad.client`).
//!
//! Owned skins run as their real SkinID, so Riot's label already shows the skin
//! name — nothing to do. An UNOWNED skin is force-loaded as **SkinID 0** (base)
//! with only the 3D model swapped in, so its label reads the base champion name
//! ("Aatrox"). To surface the skin name we repoint every string-table entry
//! whose value is exactly the champion's display name to the skin's name. That
//! renames the champion wherever it appears that game (scoreboard, tab, etc.) —
//! there's no loading-screen-only key — which is the accepted trade-off.
//!
//! Verified 2026-07-18: RST v5 = `"RST"|0x05 | u32 count | count×u64 | blob`;
//! each entry `= (offset << 38) | (hash & (2^38-1))`, offset is into the string
//! blob (null-terminated UTF-8). Round-trips byte-identically; the override
//! builds a single `Global.<locale>.wad.client` (no full-game rebuild).

use std::path::{Path, PathBuf};

use crate::skins::slog::{log_info, log_warn};

/// Folder name of the generated overlay mod (single-slot; rebuilt each pick).
pub const MOD_NAME: &str = "chud_loadscreen";
/// cslol mod manifest — mkoverlay rejects a mod folder without `META/info.json`.
const MOD_INFO_JSON: &str =
    r#"{"Author":"Chud","Description":"Loadscreen skin-name label","Name":"Chud Loadscreen","Version":"1.0.0"}"#;

const RST_SHIFT: u32 = 38;

/// Repoint every RST v5 entry whose value is exactly `champ_display` to
/// `skin_name`, appending the new string to the blob. Returns the rewritten
/// table, or `None` if it isn't the expected RST v5 or the champion name isn't
/// present (so we never ship an unchanged 18 MB file).
fn patch_champion_name(rst: &[u8], champ_display: &str, skin_name: &str) -> Option<Vec<u8>> {
    if rst.len() < 8 || &rst[0..3] != b"RST" || rst[3] != 5 {
        return None;
    }
    let count = u32::from_le_bytes(rst[4..8].try_into().ok()?) as usize;
    let ent_start = 8usize;
    let blob_start = ent_start.checked_add(count.checked_mul(8)?)?;
    if blob_start > rst.len() {
        return None;
    }
    let blob = &rst[blob_start..];
    let hmask: u64 = (1u64 << RST_SHIFT) - 1;

    let str_at = |rel: usize| -> Option<&[u8]> {
        if rel >= blob.len() {
            return None;
        }
        let end = blob[rel..].iter().position(|&b| b == 0)? + rel;
        Some(&blob[rel..end])
    };

    let target = champ_display.as_bytes();
    let mut entries: Vec<u64> = Vec::with_capacity(count);
    let mut repoint: Vec<usize> = Vec::new();
    for i in 0..count {
        let off = ent_start + i * 8;
        let v = u64::from_le_bytes(rst[off..off + 8].try_into().ok()?);
        if str_at((v >> RST_SHIFT) as usize) == Some(target) {
            repoint.push(i);
        }
        entries.push(v);
    }
    if repoint.is_empty() {
        return None;
    }

    let new_rel = blob.len() as u64;
    // The new offset must still fit the 26-bit offset field (blob << 38).
    if new_rel + skin_name.len() as u64 + 1 >= (1u64 << (64 - RST_SHIFT)) {
        return None;
    }
    for &i in &repoint {
        entries[i] = (new_rel << RST_SHIFT) | (entries[i] & hmask);
    }

    let mut out = Vec::with_capacity(rst.len() + skin_name.len() + 1);
    out.extend_from_slice(&rst[0..4]);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for v in &entries {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(blob);
    out.extend_from_slice(skin_name.as_bytes());
    out.push(0);
    Some(out)
}

/// Locate the localized global WAD (`Global.<locale>.wad.client`) and its
/// locale token, e.g. (`.../Localized/Global.en_US.wad.client`, "en_US").
fn locale_global_wad(game_dir: &Path) -> Option<(PathBuf, String)> {
    let dir = game_dir.join("DATA").join("FINAL").join("Localized");
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(locale) = name.strip_prefix("Global.").and_then(|r| r.strip_suffix(".wad.client")) {
            if !locale.is_empty() {
                return Some((entry.path(), locale.to_string()));
            }
        }
    }
    None
}

/// The raw `lol.stringtable` for `locale`, extracted from the game WAD and
/// cached under the data dir. Re-extracts when the source WAD is newer than the
/// cache (a game patch). `None` on any failure.
fn stringtable_source(wad: &Path, locale: &str) -> Option<Vec<u8>> {
    let inner_rel = format!("data/menu/{}/lol.stringtable", locale.to_lowercase());
    let cache = crate::skins::paths::data_root().join("cache").join(format!("lol_{locale}.stringtable"));

    let wad_mtime = std::fs::metadata(wad).and_then(|m| m.modified()).ok();
    if let (Ok(cm), Some(wm)) = (std::fs::metadata(&cache).and_then(|m| m.modified()), wad_mtime) {
        if cm >= wm {
            if let Ok(b) = std::fs::read(&cache) {
                return Some(b);
            }
        }
    }

    let tmp = crate::skins::paths::injection_dir().join(".st_extract");
    let _ = std::fs::remove_dir_all(&tmp);
    let wad_extract = crate::skins::injection::tools::cslol_tools_dir().join("wad-extract.exe");
    let mut cmd = std::process::Command::new(&wad_extract);
    cmd.arg(wad).arg(&tmp);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash mid-game
    }
    let ok = cmd.output().map(|o| o.status.success()).unwrap_or(false);
    let result = if ok {
        let p = tmp.join(inner_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::read(&p).ok()
    } else {
        log_warn!("[LOADSCREEN] wad-extract failed for {}", wad.display());
        None
    };
    if let Some(bytes) = &result {
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, bytes);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Build the loadscreen name-override mod under the injection mods dir: repoint
/// the champion name to `skin_name` in the localized string table so the game's
/// SkinID-0 label reads the skin name. Returns the mod folder name, or `None`
/// (best-effort — a failure never blocks the skin from injecting). Local-only;
/// no network.
pub fn build(champ_display: &str, skin_name: &str) -> Option<String> {
    if skin_name.is_empty() || champ_display.is_empty() || skin_name == champ_display {
        return None;
    }
    let game_dir = crate::skins::lcu_ext::resolve_game_dir()?;
    let (wad, locale) = locale_global_wad(&game_dir)?;
    let src = stringtable_source(&wad, &locale)?;
    let patched = match patch_champion_name(&src, champ_display, skin_name) {
        Some(p) => p,
        None => {
            log_warn!("[LOADSCREEN] '{champ_display}' not found in string table (or unsupported) - name unchanged");
            return None;
        }
    };

    let inner = format!("data/menu/{}/lol.stringtable", locale.to_lowercase());
    let mod_root = crate::skins::paths::injection_mods_dir().join(MOD_NAME);
    let meta = mod_root.join("META");
    if std::fs::create_dir_all(&meta).is_err() {
        return None;
    }
    if std::fs::write(meta.join("info.json"), MOD_INFO_JSON).is_err() {
        return None;
    }
    let dest = mod_root
        .join("WAD")
        .join(format!("Global.{locale}.wad.client"))
        .join(inner.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dest.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return None;
        }
    }
    if let Err(e) = std::fs::write(&dest, &patched) {
        log_warn!("[LOADSCREEN] string-table write failed: {e}");
        return None;
    }
    log_info!("[LOADSCREEN] name label: '{champ_display}' -> '{skin_name}' via Global.{locale}.wad.client");
    Some(MOD_NAME.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Round-trip + edit proof against the real extracted lol.stringtable.
    // Point CHUD_STRINGTABLE at a copy and run:
    //   cargo test --lib loadscreen_stringtable_patch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn loadscreen_stringtable_patch() {
        let p = std::env::var("CHUD_STRINGTABLE").expect("set CHUD_STRINGTABLE to a lol.stringtable");
        let data = std::fs::read(&p).unwrap();
        // Unchanged patch (name not present) returns None.
        assert!(patch_champion_name(&data, "ZzzNotAName", "X").is_none());
        // Real champion name repoints and the file stays parseable/larger.
        let out = patch_champion_name(&data, "Aatrox", "Prestige DRX Aatrox").expect("Aatrox present");
        assert_eq!(&out[0..4], &data[0..4], "header preserved");
        assert_eq!(out.len(), data.len() + "Prestige DRX Aatrox".len() + 1, "one appended string");
        // Every 'Aatrox' entry now resolves to the skin name.
        let count = u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize;
        let blob = &out[8 + count * 8..];
        let str_at = |rel: usize| {
            let end = blob[rel..].iter().position(|&b| b == 0).unwrap() + rel;
            &blob[rel..end]
        };
        let mut repointed = 0;
        for i in 0..count {
            let v = u64::from_le_bytes(out[8 + i * 8..8 + i * 8 + 8].try_into().unwrap());
            if str_at((v >> RST_SHIFT) as usize) == b"Prestige DRX Aatrox" {
                repointed += 1;
            }
        }
        assert!(repointed >= 1, "at least one entry repointed");
        eprintln!("repointed {repointed} entries, +{} bytes", out.len() - data.len());
    }
}
