//! `modscan-core` — a defensive scanner for League of Legends mod archives
//! (`.fantome` files, which are just renamed `.zip`s).
//!
//! Intentionally PURE: no Tauri, no networking, no filesystem writes — it only
//! reads the bytes it's handed and reports findings. Shared between the Chud
//! app and a standalone `modscan` CLI, and trivial to unit test/fuzz in isolation.
//!
//! `scan_bytes` is the only entry point: hand it a file's bytes, get a
//! `ScanReport`. Never panics, never does unbounded work — every read is capped
//! (see `MAX_*` below), so a hostile .fantome can't DoS the scanner itself.

use std::collections::BTreeMap;
use std::io::Read;

use serde::Serialize;
use sha2::{Digest, Sha256};
use zip::result::ZipError;

// ---------------------------------------------------------------------------
// Tunables
// ---------------------------------------------------------------------------

/// Zip bombs and "make the reviewer's tool choke" archives love absurd entry
/// counts. Past this we already know the answer (Malicious) — no need to
/// trust the entry count enough to allocate per-entry state for all of them.
pub const MAX_ENTRIES: usize = 50_000;

/// Sum of every entry's *declared* uncompressed size. A few KB compressed
/// can claim to expand to terabytes (classic zip bomb) — this is the
/// "would filesystem-fill the machine on naive extract" tripwire.
pub const MAX_TOTAL_UNCOMPRESSED: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB

/// Same idea as `MAX_TOTAL_UNCOMPRESSED` but for a single member — one
/// enormous entry can trip this even if the archive as a whole looks small.
pub const MAX_SINGLE_ENTRY_UNCOMPRESSED: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

/// Uncompressed/compressed ratio past which an entry is "too good to be real
/// cosmetic data" (DEFLATE tops ~1000:1 on degenerate input like zero-runs;
/// legit textures/wads never approach 200:1). Entries under 1 KiB compressed
/// are ignored — tiny files hit silly ratios without being a bomb.
pub const MAX_COMPRESSION_RATIO: u64 = 200;

/// How many leading bytes of an entry's *decompressed* stream we sniff for a
/// magic number. Bounded deliberately — we never want to decompress a whole
/// entry just to classify it.
pub const MAGIC_SNIFF_BYTES: usize = 16;

/// Bounded window (past the offset-0 sniff) searched for an executable embedded
/// BEHIND a benign header — the polyglot trick (valid image header, real PE
/// appended). Capped so a hostile entry can't make us inflate unboundedly.
pub const POLYGLOT_SCAN_BYTES: usize = 4096;

/// Max depth we recurse into nested archives. A cosmetic mod never nests; this
/// only bounds how far we chase a payload hidden in a zip-in-zip.
const MAX_NEST_DEPTH: usize = 4;

/// Max bytes inflated from a single nested-archive entry to recurse into it.
/// Bounds the classic recursive zip-bomb: a bomb layer exceeding this is
/// truncated (fails to parse) and flagged, never fully expanded.
const NESTED_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Global ceiling on ACTUAL bytes decompressed across the whole scan (not
/// per-entry). Without it, up to `MAX_ENTRIES` nested-archive members ×
/// `NESTED_MAX_BYTES` would force terabytes of real inflation — a scanner DoS.
/// Legit cosmetic mods read only a few KiB per entry, so this only ever trips a
/// decompression bomb, after which remaining entries are metadata-checked only.
const MAX_TOTAL_DECOMPRESSED: u64 = 1024 * 1024 * 1024; // 1 GiB

/// Cap on how many `unexpected-content` findings we emit for one archive.
/// A mod pack with a thousand stray files shouldn't turn into a thousand
/// findings — group by extension and stop after this many groups.
const MAX_UNEXPECTED_CONTENT_FINDINGS: usize = 20;

/// Extensions that are simply never legitimate in a cosmetic skin mod.
/// Anything here is a runnable/loadable payload on Windows regardless of
/// what the archive author *called* it.
const DANGEROUS_EXTENSIONS: &[&str] = &[
    "exe", "dll", "sys", "scr", "com", "bat", "cmd", "ps1", "psm1", "vbs", "vbe", "js", "jse",
    "wsf", "wsh", "hta", "jar", "msi", "msp", "lnk", "url", "reg", "cpl", "gadget", "inf", "pif",
    "application", "msc", "ocx", "drv", "efi",
];

/// Extensions a normal League cosmetic mod (WAD overlay, texture swap, VFX
/// tweak, etc.) is expected to contain. Anything else is `unexpected-content`
/// — not proof of malice by itself, but off-contract and worth a human look.
const COSMETIC_EXTENSIONS: &[&str] = &[
    "wad", "client", "dds", "tex", "png", "jpg", "jpeg", "tga", "skn", "skl", "scb", "sco", "anm",
    "mapgeo", "bin", "troybin", "wpk", "bnk", "wem", "ogg", "preload", "luaobj", "json", "txt",
    "subchunktoc", "dat",
];

/// Archive/compression extensions — a nested archive can hide payloads from
/// a shallow (non-recursive) scan of the outer .fantome.
const NESTED_ARCHIVE_EXTENSIONS: &[&str] =
    &["zip", "fantome", "rar", "7z", "gz", "bz2", "xz", "tar", "cab"];

/// Windows reserved device names. A path component matching one of these
/// (extension stripped, case-insensitive) can misbehave badly on extraction
/// (e.g. `WAD/CON.dds` tries to open the CON device, not a file).
const RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Overall call on an archive. Anything `Malicious` should block install;
/// `Suspicious` is a "warn the user, let them decide" tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Clean,
    Suspicious,
    Malicious,
}

/// Severity of a single finding. Kept separate from `Verdict` — a report can
/// carry Info findings alongside its final verdict without changing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Suspicious,
    Malicious,
}

/// A single detection. `entry` is the archive-relative path the finding is
/// about, when it's about one specific entry (grouped findings, or
/// whole-archive findings like `too-many-entries`, leave it `None`).
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    pub code: String,
    pub entry: Option<String>,
    pub detail: String,
}

impl Finding {
    fn new(severity: Severity, code: &str, entry: Option<String>, detail: impl Into<String>) -> Self {
        Finding { severity, code: code.to_string(), entry, detail: detail.into() }
    }
}

/// Full result of scanning one archive.
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub verdict: Verdict,
    /// Hex-encoded SHA-256 of the whole input file, regardless of whether it
    /// parsed as a zip — this is how a caller correlates a report back to a
    /// specific file even if the "not a zip" path was taken.
    pub sha256: String,
    pub file_size: u64,
    pub entry_count: usize,
    pub total_uncompressed: u64,
    pub findings: Vec<Finding>,
}

impl ScanReport {
    /// True when this archive should be blocked outright (install refused),
    /// as opposed to `Suspicious`, which is a warn-and-let-the-user-decide.
    pub fn is_blocking(&self) -> bool {
        self.verdict == Verdict::Malicious
    }

    /// Multi-line, human-readable rendering of the report — used by the CLI
    /// in non-`--json` mode and handy in logs/error messages from callers.
    pub fn human_summary(&self) -> String {
        let mut out = format!(
            "verdict: {:?}\nsha256: {}\nsize: {} bytes\nentries: {}\ntotal uncompressed: {} bytes\n",
            self.verdict, self.sha256, self.file_size, self.entry_count, self.total_uncompressed
        );
        if self.findings.is_empty() {
            out.push_str("findings: none\n");
        } else {
            out.push_str("findings:\n");
            for f in &self.findings {
                let entry = f.entry.as_deref().unwrap_or("-");
                out.push_str(&format!("  [{:?}] {} ({}): {}\n", f.severity, f.code, entry, f.detail));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Scan a whole `.fantome`/`.zip` file's bytes and return a report. Never
/// panics: any per-entry failure (corrupt entry, decompression error) is
/// downgraded to an `unreadable-entry` Info finding rather than propagated.
pub fn scan_bytes(data: &[u8]) -> ScanReport {
    scan_bytes_depth(data, 0)
}

/// `scan_bytes`, threaded with a nesting `depth` so the nested-archive
/// recursion below can bound how far it chases a payload hidden in a
/// zip-in-zip.
fn scan_bytes_depth(data: &[u8], depth: usize) -> ScanReport {
    let sha256 = hex_sha256(data);
    let file_size = data.len() as u64;

    let mut archive = match zip::ZipArchive::new(std::io::Cursor::new(data)) {
        Ok(archive) => archive,
        Err(_) => {
            // Not a valid zip. Before writing it off as a corrupt download,
            // sniff the head: a RAR/7z/PE blob renamed `.fantome` is a
            // disguised non-zip payload, not corruption — flag those Malicious
            // rather than a benign "not-a-zip".
            return non_zip_report(data, sha256, file_size);
        }
    };

    let mut findings = Vec::new();
    let entry_count = archive.len();

    if entry_count == 0 {
        findings.push(Finding::new(Severity::Suspicious, "empty-archive", None, "archive contains no entries"));
    }

    if entry_count > MAX_ENTRIES {
        findings.push(Finding::new(
            Severity::Malicious,
            "too-many-entries",
            None,
            format!("archive has {entry_count} entries, exceeding the {MAX_ENTRIES} guardrail"),
        ));
    }

    // Bound how many entries we actually iterate — an absurd entry count is
    // already a verdict on its own; we don't need to pay for per-entry work
    // on all of them to prove it further.
    let scan_limit = entry_count.min(MAX_ENTRIES);

    let mut total_uncompressed: u64 = 0;
    let mut total_uncompressed_exceeded = false;
    // Actual bytes decompressed by content reads so far, and a latch once the
    // global budget trips — bounds total scan CPU (see MAX_TOTAL_DECOMPRESSED).
    let mut total_decompressed: u64 = 0;
    let mut decompression_budget_hit = false;

    // Grouped by extension so a pack with hundreds of stray files produces
    // a handful of findings, not one per file.
    let mut unexpected_content: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut has_meta = false;
    let mut has_wad_member = false;
    let mut has_wad_dir = false;

    for i in 0..scan_limit {
        // Peek the raw directory entry first — this succeeds even for encrypted
        // members (no decryption needed), so an encrypted entry can't skip every
        // check by simply failing to open. Gives us the name + encryption flag.
        let (raw_name_peek, raw_encrypted) = match archive.by_index_raw(i) {
            Ok(raw) => (Some(raw.name().to_string()), raw.encrypted()),
            Err(_) => (None, false),
        };

        let mut entry = match archive.by_index(i) {
            Ok(entry) => entry,
            Err(err) => {
                // No cosmetic mod ships encrypted members — flag it Malicious,
                // and use the peeked name so path/extension checks still apply.
                let is_encrypted = raw_encrypted
                    || matches!(&err, ZipError::UnsupportedArchive(m) if *m == ZipError::PASSWORD_REQUIRED);
                if is_encrypted {
                    findings.push(Finding::new(
                        Severity::Malicious,
                        "encrypted-entry",
                        raw_name_peek.clone(),
                        "archive member is password-protected/encrypted — its contents can't be scanned, and no legitimate cosmetic mod ships encrypted members",
                    ));
                    if let Some(name) = &raw_name_peek {
                        let normalized = name.replace('\\', "/");
                        if let Some(reason) = path_traversal_reason(&normalized) {
                            findings.push(Finding::new(Severity::Malicious, "path-traversal", Some(name.clone()), reason));
                        }
                        if let Some(e) = extension_of(&normalized) {
                            if DANGEROUS_EXTENSIONS.contains(&e.as_str()) {
                                findings.push(Finding::new(
                                    Severity::Malicious,
                                    "dangerous-extension",
                                    Some(name.clone()),
                                    format!("extension \"{e}\" is a runnable/loadable payload type, never legitimate in a cosmetic mod"),
                                ));
                            }
                        }
                    }
                } else {
                    findings.push(Finding::new(
                        Severity::Info,
                        "unreadable-entry",
                        raw_name_peek,
                        format!("entry {i} could not be read: {err}"),
                    ));
                }
                continue;
            }
        };

        let raw_name = entry.name().to_string();
        let normalized = raw_name.replace('\\', "/");
        let is_dir = entry.is_dir();

        // --- 1. Path safety (zip-slip, absolute paths, ADS, device names) ---
        let traversal_reason = path_traversal_reason(&normalized);
        let enclosed_rejected = entry.enclosed_name().is_none();
        let is_traversal = traversal_reason.is_some() || enclosed_rejected;
        if let Some(reason) = &traversal_reason {
            findings.push(Finding::new(Severity::Malicious, "path-traversal", Some(raw_name.clone()), reason.clone()));
        } else if enclosed_rejected {
            findings.push(Finding::new(
                Severity::Malicious,
                "path-traversal",
                Some(raw_name.clone()),
                "enclosed_name() rejected this path as unsafe",
            ));
        }

        // --- 2. Symlink entries (mode bits, unix_mode is None on non-unix zips) ---
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                findings.push(Finding::new(
                    Severity::Malicious,
                    "symlink",
                    Some(raw_name.clone()),
                    "entry is a symlink — can point outside the extraction root",
                ));
            }
        }

        let ext = extension_of(&normalized);
        let nested_by_ext = ext.as_deref().is_some_and(|e| NESTED_ARCHIVE_EXTENSIONS.contains(&e));

        // --- 3. Dangerous extension ---
        let dangerous_ext = ext.as_deref().is_some_and(|e| DANGEROUS_EXTENSIONS.contains(&e));
        if dangerous_ext {
            findings.push(Finding::new(
                Severity::Malicious,
                "dangerous-extension",
                Some(raw_name.clone()),
                format!("extension \"{}\" is a runnable/loadable payload type, never legitimate in a cosmetic mod", ext.as_deref().unwrap_or("")),
            ));
        }

        // Declared metadata captured BEFORE the content read (which borrows
        // `entry`); used by the ratio-bomb guard and the size-lie check.
        let size = entry.size();
        let compressed_size = entry.compressed_size();

        // --- 4. Content inspection (bounded read; never for directories) ---
        // For a possible nested archive we read enough (bounded) to recurse
        // into it; otherwise just a small polyglot window. Either way capped so
        // a hostile entry can't make us inflate unboundedly.
        let mut disguised = false;
        let mut nested_by_magic = false;
        let mut content: Vec<u8> = Vec::new();
        if !is_dir && !decompression_budget_hit {
            let read_cap: u64 = if nested_by_ext { NESTED_MAX_BYTES } else { POLYGLOT_SCAN_BYTES as u64 };
            // Clamp to the remaining GLOBAL decompression budget so the whole
            // scan can't be forced into terabytes of inflation across entries.
            let read_cap = read_cap.min(MAX_TOTAL_DECOMPRESSED.saturating_sub(total_decompressed));
            let mut limited = (&mut entry).take(read_cap);
            match limited.read_to_end(&mut content) {
                Ok(_) => {
                    total_decompressed += content.len() as u64;
                    if total_decompressed >= MAX_TOTAL_DECOMPRESSED {
                        decompression_budget_hit = true;
                        findings.push(Finding::new(
                            Severity::Suspicious,
                            "scan-budget-exceeded",
                            None,
                            format!("archive forced past the {MAX_TOTAL_DECOMPRESSED}-byte decompression budget — remaining entries were not content-scanned"),
                        ));
                    }
                    // Disguise at offset 0 (e.g. a PE named `.dds`).
                    if let Some(kind) = sniff_executable_magic(&content) {
                        disguised = true;
                        findings.push(Finding::new(
                            Severity::Malicious,
                            "disguised-executable",
                            Some(raw_name.clone()),
                            format!("entry content sniffs as {kind}, regardless of its \"{}\" extension", ext.as_deref().unwrap_or("(none)")),
                        ));
                    }
                    // Polyglot: a real Windows PE embedded BEHIND a benign
                    // header (the offset-0 sniff misses this). Bounded window.
                    if !disguised {
                        let window = &content[..content.len().min(POLYGLOT_SCAN_BYTES)];
                        if let Some(off) = find_embedded_pe(window) {
                            disguised = true;
                            findings.push(Finding::new(
                                Severity::Malicious,
                                "embedded-executable",
                                Some(raw_name.clone()),
                                format!("a Windows PE executable is embedded at byte offset {off}, behind a benign-looking header"),
                            ));
                        }
                    }
                    nested_by_magic = sniff_nested_archive_magic(&content);
                    // Size-lie: we produced more decompressed bytes than the
                    // central directory declares, so the metadata the ratio /
                    // total guards trust is unreliable. (Inert if the reader
                    // caps at the declared size; free defense-in-depth if not.)
                    if (content.len() as u64) > size {
                        findings.push(Finding::new(
                            Severity::Suspicious,
                            "size-mismatch",
                            Some(raw_name.clone()),
                            format!("decompressed to more than the declared {size} bytes — archive size metadata is unreliable"),
                        ));
                    }
                }
                Err(err) => {
                    findings.push(Finding::new(
                        Severity::Info,
                        "unreadable-entry",
                        Some(raw_name.clone()),
                        format!("could not read entry data for content scan: {err}"),
                    ));
                }
            }
        }

        // --- 5. Compression-ratio bomb (declared metadata) ---
        // `>= 1024` already guarantees `compressed_size > 0`, so the division is safe.
        let ratio_bomb = (compressed_size >= 1024 && size / compressed_size > MAX_COMPRESSION_RATIO)
            || size > MAX_SINGLE_ENTRY_UNCOMPRESSED;
        if ratio_bomb {
            findings.push(Finding::new(
                Severity::Suspicious,
                "compression-bomb-entry",
                Some(raw_name.clone()),
                format!(
                    "uncompressed size {size} bytes from {compressed_size} bytes compressed \
                     — decompression-bomb ratio or absolute size guardrail tripped"
                ),
            ));
        }

        // --- 6. Nested archive: recurse (bounded depth) so a payload hidden in
        // a zip-in-zip can't slip through a shallow scan. Nesting is itself
        // off-contract for a cosmetic mod, so it always warns; a payload found
        // inside escalates the whole report to Malicious. ---
        if nested_by_magic || nested_by_ext {
            findings.push(Finding::new(
                Severity::Suspicious,
                "nested-archive",
                Some(raw_name.clone()),
                "entry is a nested archive — its contents are scanned recursively below",
            ));
            if depth + 1 >= MAX_NEST_DEPTH {
                findings.push(Finding::new(
                    Severity::Malicious,
                    "nested-archive-depth",
                    Some(raw_name.clone()),
                    "nested archive exceeds the maximum scan depth — cannot be fully audited",
                ));
            } else if !content.is_empty() {
                match zip::ZipArchive::new(std::io::Cursor::new(&content[..])) {
                    Ok(_) => {
                        let inner = scan_bytes_depth(&content, depth + 1);
                        for mut f in inner.findings {
                            if f.severity == Severity::Info {
                                continue;
                            }
                            f.entry = Some(match &f.entry {
                                Some(e) => format!("{raw_name}!/{e}"),
                                None => raw_name.clone(),
                            });
                            findings.push(f);
                        }
                    }
                    Err(_) => {
                        // Nested magic/extension but not a parseable zip (rar/7z/
                        // tar, or a truncated bomb layer) — an opaque payload we
                        // can't audit. Never legitimate in a cosmetic mod.
                        findings.push(Finding::new(
                            Severity::Malicious,
                            "opaque-archive",
                            Some(raw_name.clone()),
                            "entry is a nested archive in a format this scanner cannot open (or a truncated bomb layer) — its contents cannot be audited",
                        ));
                    }
                }
            }
        }

        // --- 7. Content-type classification (skip dirs and anything already flagged malicious) ---
        if !is_dir && !is_traversal && !dangerous_ext && !disguised {
            let is_cosmetic = match ext.as_deref() {
                Some(e) => COSMETIC_EXTENSIONS.contains(&e),
                // No extension is only fine for hashed WAD members (e.g.
                // `WAD/3AB2...`); a bare-name file anywhere else is unusual.
                None => path_has_component(&normalized, "wad"),
            };
            if !is_cosmetic {
                let key = ext.clone().unwrap_or_else(|| "(no extension)".to_string());
                unexpected_content.entry(key).or_default().push(raw_name.clone());
            }
        }

        // --- running total for the zip-bomb-total guardrail ---
        if !total_uncompressed_exceeded {
            total_uncompressed = total_uncompressed.saturating_add(size);
            if total_uncompressed > MAX_TOTAL_UNCOMPRESSED {
                total_uncompressed_exceeded = true;
                findings.push(Finding::new(
                    Severity::Suspicious,
                    "zip-bomb-total",
                    None,
                    format!("running total of declared uncompressed sizes exceeded {MAX_TOTAL_UNCOMPRESSED} bytes"),
                ));
            }
        }

        // --- structure sanity bookkeeping ---
        if path_has_component(&normalized, "meta") {
            has_meta = true;
        }
        if path_has_component(&normalized, "wad") {
            has_wad_dir = true;
        }
        if ext.as_deref() == Some("wad") || normalized.to_lowercase().ends_with(".wad.client") {
            has_wad_member = true;
        }
    }

    // --- 7 (cont'd). Emit the grouped unexpected-content findings, capped ---
    let total_groups = unexpected_content.len();
    for (ext, entries) in unexpected_content.into_iter().take(MAX_UNEXPECTED_CONTENT_FINDINGS) {
        let mut example_entries = entries.clone();
        example_entries.truncate(5);
        let suffix = if entries.len() > example_entries.len() { ", ..." } else { "" };
        findings.push(Finding::new(
            Severity::Suspicious,
            "unexpected-content",
            None,
            format!(
                "{} entr{} with extension \"{ext}\" not in the cosmetic-mod allowlist: {}{suffix}",
                entries.len(),
                if entries.len() == 1 { "y" } else { "ies" },
                example_entries.join(", "),
            ),
        ));
    }
    if total_groups > MAX_UNEXPECTED_CONTENT_FINDINGS {
        findings.push(Finding::new(
            Severity::Suspicious,
            "unexpected-content",
            None,
            format!("{} more unexpected extensions omitted for brevity", total_groups - MAX_UNEXPECTED_CONTENT_FINDINGS),
        ));
    }

    // --- 8. Structure sanity ---
    if !has_meta && !has_wad_member && !has_wad_dir {
        findings.push(Finding::new(
            Severity::Info,
            "no-mod-structure",
            None,
            "no META/ entry, WAD/ directory, or .wad(.client) member found — doesn't look like a normal mod",
        ));
    }

    let verdict = if findings.iter().any(|f| f.severity == Severity::Malicious) {
        Verdict::Malicious
    } else if findings.iter().any(|f| f.severity == Severity::Suspicious) {
        Verdict::Suspicious
    } else {
        Verdict::Clean
    };

    ScanReport { verdict, sha256, file_size, entry_count, total_uncompressed, findings }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hex_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Returns `Some(reason)` if `normalized` (already `\`→`/` normalized) is
/// unsafe to extract, covering the zip-slip / absolute-path / ADS / device
/// name / trailing-space-or-dot tricks. Order matters only for which
/// `detail` string a caller sees — any hit is Malicious regardless.
fn path_traversal_reason(normalized: &str) -> Option<String> {
    if normalized.split('/').any(|c| c == "..") {
        return Some("path contains a \"..\" component (zip-slip)".to_string());
    }
    if normalized.starts_with('/') {
        return Some("absolute path".to_string());
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Some("Windows drive-letter prefix".to_string());
    }
    if normalized.contains(':') {
        return Some("contains ':' — possible NTFS alternate data stream".to_string());
    }
    for component in normalized.split('/').filter(|c| !c.is_empty()) {
        let base = component.split('.').next().unwrap_or(component);
        if RESERVED_NAMES.contains(&base.to_uppercase().as_str()) {
            return Some(format!("path component \"{component}\" is a reserved Windows device name"));
        }
        if component.ends_with(' ') || component.ends_with('.') {
            return Some(format!(
                "path component \"{component}\" has a trailing space/dot (Windows path-normalization trick)"
            ));
        }
    }
    None
}

/// Lowercased final extension of a (already `/`-normalized) archive path, or
/// `None` if the final path component has no extension.
fn extension_of(normalized: &str) -> Option<String> {
    let basename = normalized.rsplit('/').next().unwrap_or(normalized);
    std::path::Path::new(basename).extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase())
}

/// True if any path component (case-insensitive) equals `name`, e.g.
/// `path_has_component("WAD/Foo.wad.client", "wad")` is true.
fn path_has_component(normalized: &str, name: &str) -> bool {
    normalized.split('/').any(|c| c.eq_ignore_ascii_case(name))
}

/// Sniff `buf` (up to `MAGIC_SNIFF_BYTES` of an entry's decompressed head)
/// for known executable/script magic numbers. This is the check that
/// catches a payload renamed to look cosmetic (e.g. `Splash.dds` that is
/// actually a Windows PE).
fn sniff_executable_magic(buf: &[u8]) -> Option<&'static str> {
    if buf.starts_with(&[0x4D, 0x5A]) {
        return Some("a PE (MZ) executable");
    }
    if buf.starts_with(&[0x7F, 0x45, 0x4C, 0x46]) {
        return Some("an ELF executable");
    }
    if buf.starts_with(&[0xFE, 0xED, 0xFA, 0xCE])
        || buf.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || buf.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
        || buf.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
        || buf.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
    {
        return Some("a Mach-O executable");
    }
    if buf.starts_with(&[0x23, 0x21]) {
        return Some("a script with a shebang (#!)");
    }
    if buf.starts_with(&[0x4C, 0x00, 0x00, 0x00]) {
        return Some("a Windows shortcut (.lnk header)");
    }
    None
}

/// Sniff `buf` for a nested-archive magic number, independent of the outer
/// entry's extension (catches an archive renamed to look like cosmetic data).
fn sniff_nested_archive_magic(buf: &[u8]) -> bool {
    buf.starts_with(&[0x50, 0x4B, 0x03, 0x04]) // PK\x03\x04
        || buf.starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A]) // Rar!\x1a
        || buf.starts_with(&[0x37, 0x7A, 0xBC, 0xAF]) // 7z\xbc\xaf
}

/// Search `buf` for a Windows PE embedded BEHIND a benign header (the polyglot
/// trick). Structural match — locate an `MZ`, follow its `e_lfanew` to a
/// `PE\0\0` signature — so a stray "MZ" in texture data doesn't false-positive.
/// Returns the offset of the `MZ`. Bounded by `buf.len()` (caller passes a
/// capped window).
fn find_embedded_pe(buf: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    // Need at least MZ + the e_lfanew field (ends at 0x40) to validate.
    while i + 0x40 <= buf.len() {
        if buf[i] == 0x4D && buf[i + 1] == 0x5A {
            let lfanew_off = i + 0x3C;
            let e_lfanew = u32::from_le_bytes([
                buf[lfanew_off],
                buf[lfanew_off + 1],
                buf[lfanew_off + 2],
                buf[lfanew_off + 3],
            ]) as usize;
            if let Some(pe_off) = i.checked_add(e_lfanew) {
                if pe_off + 4 <= buf.len() && &buf[pe_off..pe_off + 4] == b"PE\x00\x00" {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Build the report for input that isn't a parseable zip. Sniffs the head for a
/// disguised non-zip payload (PE/RAR/7z renamed `.fantome`) and escalates those
/// to Malicious; a genuinely-unrecognized blob stays a benign "not-a-zip".
fn non_zip_report(data: &[u8], sha256: String, file_size: u64) -> ScanReport {
    let head = &data[..data.len().min(POLYGLOT_SCAN_BYTES)];
    let finding = if let Some(kind) = sniff_executable_magic(head) {
        Finding::new(
            Severity::Malicious,
            "disguised-executable",
            None,
            format!("file is not a zip but its content sniffs as {kind} — a disguised payload renamed to look like a mod"),
        )
    } else if find_embedded_pe(head).is_some() {
        Finding::new(
            Severity::Malicious,
            "embedded-executable",
            None,
            "file is not a zip but contains an embedded Windows PE executable",
        )
    } else if head.starts_with(&[0x52, 0x61, 0x72, 0x21, 0x1A]) || head.starts_with(&[0x37, 0x7A, 0xBC, 0xAF]) {
        Finding::new(
            Severity::Malicious,
            "disguised-archive-format",
            None,
            "file is a RAR/7z archive renamed to look like a .fantome — an opaque, unauditable format that is never a legitimate cosmetic mod",
        )
    } else {
        Finding::new(Severity::Info, "not-a-zip", None, "file could not be opened as a zip archive")
    };
    let verdict = match finding.severity {
        Severity::Malicious => Verdict::Malicious,
        _ => Verdict::Suspicious,
    };
    ScanReport { verdict, sha256, file_size, entry_count: 0, total_uncompressed: 0, findings: vec![finding] }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// Build a `.fantome`-shaped zip in memory from `(name, bytes)` pairs,
    /// using STORE (no compression) so ratio-based tests get predictable
    /// compressed sizes.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn clean_cosmetic_mod_is_clean() {
        let bytes = build_zip(&[
            ("META/info.json", br#"{"Name":"Test Skin"}"#),
            ("WAD/Foo.wad.client", b"fake wad bytes"),
            ("WAD/texture.dds", b"fake dds bytes"),
        ]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Clean, "{:?}", report.findings);
        assert!(!report.sha256.is_empty());
    }

    #[test]
    fn zip_slip_is_malicious() {
        let bytes = build_zip(&[("../../evil.txt", b"pwned")]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious);
        assert!(report.findings.iter().any(|f| f.code == "path-traversal"));
    }

    #[test]
    fn absolute_and_drive_and_ads_paths_are_malicious() {
        for name in ["/etc/passwd", "C:\\x", "foo.dds:bad"] {
            let bytes = build_zip(&[(name, b"data")]);
            let report = scan_bytes(&bytes);
            assert_eq!(report.verdict, Verdict::Malicious, "name={name} findings={:?}", report.findings);
            assert!(
                report.findings.iter().any(|f| f.code == "path-traversal"),
                "name={name} findings={:?}",
                report.findings
            );
        }
    }

    #[test]
    fn disguised_executable_is_malicious() {
        let mut payload = vec![b'M', b'Z'];
        payload.extend_from_slice(&[0u8; 32]);
        let bytes = build_zip(&[("WAD/Splash.dds", &payload)]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious);
        assert!(report.findings.iter().any(|f| f.code == "disguised-executable"));
    }

    #[test]
    fn dangerous_extension_is_malicious() {
        let bytes = build_zip(&[("helper.exe", b"MZ fake pe")]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious);
        assert!(report.findings.iter().any(|f| f.code == "dangerous-extension"));
    }

    #[test]
    fn lua_entry_is_suspicious_unexpected_content() {
        let bytes = build_zip(&[("scripts/hook.lua", b"print('hi')")]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Suspicious);
        assert!(report.findings.iter().any(|f| f.code == "unexpected-content"));
    }

    #[test]
    fn nested_archive_is_suspicious() {
        let inner = build_zip(&[("a.txt", b"hi")]);
        let bytes = build_zip(&[("inner.zip", &inner)]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Suspicious);
        assert!(report.findings.iter().any(|f| f.code == "nested-archive"));
    }

    #[test]
    fn compression_bomb_entry_is_suspicious() {
        // STORE doesn't compress at all, so DEFLATE is needed here for a real ratio.
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("WAD/zeros.bin", opts).unwrap();
        let zeros = vec![0u8; 5 * 1024 * 1024];
        std::io::Write::write_all(&mut writer, &zeros).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Suspicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "compression-bomb-entry"));
    }

    #[test]
    fn reserved_device_name_is_malicious() {
        let bytes = build_zip(&[("WAD/CON.dds", b"data")]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious);
        assert!(report.findings.iter().any(|f| f.code == "path-traversal"));
    }

    #[test]
    fn not_a_zip_is_suspicious_but_still_hashed() {
        let bytes = b"this is definitely not a zip file".to_vec();
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Suspicious);
        assert!(report.findings.iter().any(|f| f.code == "not-a-zip"));
        assert_eq!(report.sha256.len(), 64);
    }

    #[test]
    fn sha256_is_correct_and_stable() {
        // sha256("hello") — a well-known test vector.
        let report = scan_bytes(b"hello");
        assert_eq!(report.sha256, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    /// Minimal blob that `find_embedded_pe` accepts: `MZ` + `e_lfanew`(0x40) +
    /// `PE\0\0` at 0x40. Not a runnable PE, just the structural signature.
    fn fake_pe() -> Vec<u8> {
        let mut pe = vec![0u8; 0x44];
        pe[0] = b'M';
        pe[1] = b'Z';
        pe[0x3C] = 0x40; // e_lfanew = 0x40 (LE)
        pe[0x40] = b'P';
        pe[0x41] = b'E';
        pe
    }

    #[test]
    fn encrypted_entry_is_malicious() {
        // C1: an encrypted member used to skip EVERY check and yield Clean.
        use std::io::Write as _;
        use zip::AesMode;
        let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().with_aes_encryption(AesMode::Aes256, "secret");
        writer.start_file("WAD/secret.wad.client", opts).unwrap();
        writer.write_all(b"hidden payload").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "encrypted-entry"), "{:?}", report.findings);
    }

    #[test]
    fn polyglot_embedded_pe_is_malicious() {
        // C3: a PE hidden PAST a benign header (offset-0 sniff misses it).
        let mut payload = vec![b'A'; 128];
        payload.extend_from_slice(&fake_pe());
        let bytes = build_zip(&[("WAD/Splash.dds", &payload)]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "embedded-executable"), "{:?}", report.findings);
    }

    #[test]
    fn nested_archive_hiding_exe_is_malicious() {
        // C2: a payload inside a zip-in-zip used to only warn (click-through).
        let inner = build_zip(&[("helper.exe", b"MZ fake pe")]);
        let bytes = build_zip(&[("WAD/pack.zip", &inner)]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(
            report.findings.iter().any(|f| f.entry.as_deref().is_some_and(|e| e.contains("!/"))),
            "nested finding should be path-prefixed: {:?}",
            report.findings
        );
    }

    #[test]
    fn opaque_nested_format_is_malicious() {
        // A nested-archive-EXTENSION entry that isn't a parseable zip (rar/7z/
        // truncated bomb) can't be audited → Malicious, not a click-through.
        let bytes = build_zip(&[("WAD/inner.rar", b"Rar!\x1a\x07\x00 not really a rar body")]);
        let report = scan_bytes(&bytes);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "opaque-archive"), "{:?}", report.findings);
    }

    #[test]
    fn non_zip_disguised_pe_is_malicious() {
        // H7: a raw PE renamed .fantome used to return a benign "not-a-zip".
        let mut blob = vec![b'M', b'Z'];
        blob.extend_from_slice(&[0u8; 64]);
        let report = scan_bytes(&blob);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "disguised-executable"), "{:?}", report.findings);
    }

    #[test]
    fn non_zip_disguised_rar_is_malicious() {
        let mut blob = vec![0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
        blob.extend_from_slice(&[0u8; 32]);
        let report = scan_bytes(&blob);
        assert_eq!(report.verdict, Verdict::Malicious, "{:?}", report.findings);
        assert!(report.findings.iter().any(|f| f.code == "disguised-archive-format"), "{:?}", report.findings);
    }

    #[test]
    fn genuinely_corrupt_file_still_benign_not_a_zip() {
        // The H7 escalation must NOT false-positive on an actually-corrupt
        // download with no executable/archive signature.
        let report = scan_bytes(b"this is definitely not a zip file");
        assert_eq!(report.verdict, Verdict::Suspicious);
        assert!(report.findings.iter().any(|f| f.code == "not-a-zip"));
    }
}
