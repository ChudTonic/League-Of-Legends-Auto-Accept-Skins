//! mod-tools `mkoverlay`/`runoverlay` wrapper — ported from
//! `injection\overlay\overlay_manager.py` (`OverlayManager.mk_run_overlay`).
//!
//! THE CLI CONTRACT (`docs/SKINS_PORT.md` §2/§3) is preserved
//! character-for-character — these argv strings are a wire contract with
//! cslol's `mod-tools.exe`, not our code to reshape:
//!   `mkoverlay <mods_dir> <overlay_dir> --game:<gpath> --mods:<a>/<b> --noTFT --ignoreConflict`
//!   `runoverlay <overlay_dir> <overlay_dir>/cslol-config.json --game:<gpath> --opts:configless`
//!
//! Security note carried over from the Python original: every argv above is
//! built from trusted internal paths/config — no user-controlled input ever
//! reaches these commands directly.

#![allow(dead_code)] // consumed by S3+ (injector wiring)

use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, SetPriorityClass, HIGH_PRIORITY_CLASS, PROCESS_SET_INFORMATION};

use crate::skins::injection::game_monitor::GameMonitor;
use crate::skins::injection::process::SharedOverlayProcess;
use crate::skins::injection::tools::{hide_directory_recursive, ToolPaths};
use crate::skins::slog::{log_error, log_info, log_warn};

/// `CREATE_NO_WINDOW` (0x08000000) — hides the console window mod-tools.exe
/// would otherwise flash open (ported from `subprocess.CREATE_NO_WINDOW`).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Boost the short-lived `mkoverlay` process's priority during injection
/// setup (ported from `config.ENABLE_MKOVERLAY_PRIORITY_BOOST`).
pub const ENABLE_MKOVERLAY_PRIORITY_BOOST: bool = true;
/// `runoverlay` runs for the entire game session; boosting its priority
/// would compete with the game for CPU and hurt performance (ported
/// verbatim from `config.ENABLE_RUNOVERLAY_PRIORITY_BOOST`, which the
/// Python original ships `false` — asymmetric with the mkoverlay boost above).
pub const ENABLE_RUNOVERLAY_PRIORITY_BOOST: bool = false;

/// `subprocess.Popen(..., timeout=120)`'s default mkoverlay timeout (ported
/// verbatim from `OverlayManager.mk_run_overlay`'s `timeout: int = 120`).
const MKOVERLAY_TIMEOUT: Duration = Duration::from_secs(120);
/// `PROCESS_MONITOR_SLEEP_S` — poll interval while babysitting the running
/// `runoverlay` child.
const PROCESS_MONITOR_SLEEP: Duration = Duration::from_millis(500);

/// Create the overlay (`mkoverlay`) and run it (`runoverlay`), resuming the
/// suspended game exactly when `runoverlay` starts (ported from
/// `OverlayManager.mk_run_overlay`).
///
/// Returns `Ok(0)` on success, or `Ok(<code>)` mirroring one of Python's
/// sentinel return codes (`127` missing tool, `124` mkoverlay timeout, `1`
/// general error, or the child's own nonzero exit code) — wrapped in
/// `Result` per the S3 interface contract, but every failure path Python
/// handled without raising stays an `Ok` here too; `Err` is reserved for
/// setup failures Python didn't model as a return code (e.g. failing to
/// create the overlay directory).
pub fn mk_run_overlay(
    tools: &ToolPaths,
    mods_dir: &Path,
    overlay_dir: &Path,
    game_dir: &Path,
    mod_names: &[String],
    process: &SharedOverlayProcess,
    game_monitor: &mut GameMonitor,
) -> Result<i32, String> {
    if !tools.modtools.exists() {
        log_error!("[INJECTOR] Missing mod-tools.exe in {}", tools.modtools.display());
        return Ok(127);
    }

    std::fs::create_dir_all(overlay_dir)
        .map_err(|e| format!("failed to create overlay directory {}: {e}", overlay_dir.display()))?;

    let names_str = mod_names.join("/");
    let gpath = game_dir.display().to_string();

    log_info!(
        "[INJECT] Creating overlay: {} mkoverlay {} {} --game:{gpath} --mods:{names_str} --noTFT --ignoreConflict",
        tools.modtools.display(),
        mods_dir.display(),
        overlay_dir.display()
    );

    let mkoverlay_start = Instant::now();
    let mut child = match Command::new(&tools.modtools)
        .arg("mkoverlay")
        .arg(mods_dir)
        .arg(overlay_dir)
        .arg(format!("--game:{gpath}"))
        .arg(format!("--mods:{names_str}"))
        .arg("--noTFT")
        .arg("--ignoreConflict")
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_error!("[INJECT] mkoverlay error: {e} - monitor will auto-resume if needed");
            return Ok(1);
        }
    };

    if ENABLE_MKOVERLAY_PRIORITY_BOOST {
        boost_priority(child.id());
    }

    // Drain stdout+stderr on separate threads — CSLOL's logi() may write to
    // stdout, and reading only one pipe risks filling the other's buffer and
    // deadlocking the child (ported from overlay_manager.py's read_output
    // threads).
    let stdout_pipe = child.stdout.take().expect("mkoverlay spawned with piped stdout");
    let stderr_pipe = child.stderr.take().expect("mkoverlay spawned with piped stderr");
    let stdout_thread = std::thread::spawn(move || drain_pipe(stdout_pipe));
    let stderr_thread = std::thread::spawn(move || drain_pipe(stderr_pipe));

    let deadline = mkoverlay_start + MKOVERLAY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                log_error!("[INJECT] mkoverlay error: {e} - monitor will auto-resume if needed");
                return Ok(1);
            }
        }
    };

    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        let mut all_lines = stdout_thread.join().unwrap_or_default();
        all_lines.extend(stderr_thread.join().unwrap_or_default());
        if all_lines.is_empty() {
            log_warn!("[INJECT] mkoverlay timeout - no output captured");
        } else {
            let tail_start = all_lines.len().saturating_sub(10);
            log_warn!("[INJECT] mkoverlay timeout - last output: {}", all_lines[tail_start..].join("; "));
        }
        return Ok(124);
    };
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let mkoverlay_duration = mkoverlay_start.elapsed();

    if !status.success() {
        let code = status.code().unwrap_or(1);
        log_error!("[INJECT] mkoverlay failed with return code: {code}");
        return Ok(code);
    }

    log_info!("[INJECT] mkoverlay completed in {:.2}s", mkoverlay_duration.as_secs_f64());

    // Wipe extracted skin files now that mkoverlay is done with them, and
    // hide the overlay files so they can't be easily browsed.
    wipe_dir_contents(mods_dir);
    hide_directory_recursive(overlay_dir);

    log_info!("[INJECT] mkoverlay done - keeping game frozen until runoverlay starts");

    // ---- runoverlay ----
    let cfg = overlay_dir.join("cslol-config.json");
    log_info!("[INJECT] Running overlay");

    let run_child = match Command::new(&tools.modtools)
        .arg("runoverlay")
        .arg(overlay_dir)
        .arg(&cfg)
        .arg(format!("--game:{gpath}"))
        .arg("--opts:configless")
        .creation_flags(CREATE_NO_WINDOW)
        // Don't capture stdout/stderr — DEVNULL avoids the same pipe-buffer
        // deadlock risk as mkoverlay, but runoverlay's session is too
        // long-lived to babysit with drain threads.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log_error!("[INJECT] runoverlay error: {e}");
            return Ok(1);
        }
    };

    if ENABLE_RUNOVERLAY_PRIORITY_BOOST {
        boost_priority(run_child.id());
    }

    {
        let mut guard = process.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(run_child);
    }

    // Resume game NOW - runoverlay started, game can load while runoverlay hooks in.
    log_info!("[INJECT] runoverlay started - resuming game");
    game_monitor.resume();

    // Monitor process with no timeout - overlay runs until explicitly
    // killed (via process::stop_overlay_process / kill_all_runoverlay_processes)
    // or the game ends.
    loop {
        let done = {
            let mut guard = process.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_mut() {
                Some(c) => matches!(c.try_wait(), Ok(Some(_))),
                // Cleared externally (e.g. ChampSelect exit killed it) — done.
                None => true,
            }
        };
        if done {
            break;
        }
        std::thread::sleep(PROCESS_MONITOR_SLEEP);
    }

    let exit_code = {
        let mut guard = process.lock().unwrap_or_else(|e| e.into_inner());
        let code = guard.as_mut().and_then(|c| c.try_wait().ok().flatten()).and_then(|s| s.code()).unwrap_or(0);
        *guard = None;
        code
    };

    wipe_dir(overlay_dir);

    if exit_code != 0 {
        log_error!("[INJECT] runoverlay failed with return code: {exit_code}");
    } else {
        log_info!("[INJECT] runoverlay completed successfully");
    }
    Ok(exit_code)
}

fn drain_pipe(pipe: impl Read) -> Vec<String> {
    BufReader::new(pipe).lines().map_while(Result::ok).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Delete extracted skin files immediately after mkoverlay consumes them
/// (ported from `OverlayManager._wipe_mods_dir`).
fn wipe_dir_contents(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    log_info!("[INJECT] Wiped mods directory after mkoverlay");
}

/// Delete overlay WAD files after runoverlay finishes, recreating the empty
/// directory (ported from `OverlayManager._wipe_overlay_dir`).
fn wipe_dir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::create_dir_all(dir);
    log_info!("[INJECT] Wiped overlay directory after game ended");
}

/// Boost `pid`'s priority class to `HIGH_PRIORITY_CLASS` (ported from
/// `psutil.Process.nice(psutil.HIGH_PRIORITY_CLASS)`). Best-effort — mirrors
/// Python swallowing the exception and just logging.
fn boost_priority(pid: u32) {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) else {
            log_warn!("[INJECT] Could not open process {pid} to boost priority");
            return;
        };
        if SetPriorityClass(handle, HIGH_PRIORITY_CLASS).is_err() {
            log_warn!("[INJECT] Could not boost process priority (PID={pid})");
        } else {
            log_info!("[INJECT] Boosted process priority (PID={pid})");
        }
        let _ = CloseHandle(handle);
    }
}
