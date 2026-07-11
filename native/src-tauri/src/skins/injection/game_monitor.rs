//! Game process suspend/resume monitor (ported from
//! `injection\game\game_monitor.py::GameMonitor`).
//!
//! During champion select the Riot client launches `League of Legends.exe`
//! early; this monitor suspends it the moment it appears so cslol's overlay
//! can hook file I/O before the game finishes loading assets, then resumes it
//! the instant `runoverlay` starts (see `overlay::mk_run_overlay`).
//!
//! Suspension uses the undocumented whole-process `NtSuspendProcess` /
//! `NtResumeProcess` `ntdll` exports (the safe `windows` crate exposes only
//! per-*thread* `SuspendThread`/`ResumeThread`). This is the single most
//! safety-critical operation in the app: a suspended game that never resumes
//! freezes the user's client forever, so resume is defended four ways —
//! (1) the unconditional auto-resume timeout in the watcher loop,
//! (2) `resume()`/`resume_if_suspended()`/`stop()` all resume a still-held
//! process, (3) the resume handle is stored so resuming never depends on
//! re-finding the process, and (4) a `Drop` guard resumes on teardown.

#![allow(dead_code)] // some entry points are consumed by S5 (trigger) wiring

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{ProcessesToUpdate, System};
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};

use crate::skins::slog::{log_error, log_info};

// Undocumented `ntdll` whole-process suspend/resume. Linked directly against
// `ntdll.lib` (always present in the Windows SDK). `NTSTATUS` return: 0
// (`STATUS_SUCCESS`) means the call succeeded.
#[link(name = "ntdll")]
extern "system" {
    fn NtSuspendProcess(process_handle: HANDLE) -> i32;
    fn NtResumeProcess(process_handle: HANDLE) -> i32;
}

const GAME_EXE_NAME: &str = "league of legends.exe";
/// Steady-state poll while hunting for the game process
/// (`PERSISTENT_MONITOR_CHECK_INTERVAL_S`).
const HUNT_INTERVAL: Duration = Duration::from_millis(50);
/// Steady-state poll once suspended, just waiting for `runoverlay`
/// (`PERSISTENT_MONITOR_IDLE_INTERVAL_S`).
const IDLE_INTERVAL: Duration = Duration::from_millis(100);
/// Rapid startup checks to catch the game the moment it launches (the client
/// can spawn it before `start()` is even called).
const RAPID_CHECKS: u32 = 10;
const RAPID_INTERVAL: Duration = Duration::from_millis(5);
const RESUME_MAX_ATTEMPTS: u32 = 3; // GAME_RESUME_MAX_ATTEMPTS
const RESUME_VERIFY_WAIT: Duration = Duration::from_millis(100); // GAME_RESUME_VERIFICATION_WAIT_S
const DEFAULT_AUTO_RESUME_SECS: f64 = 60.0;

/// Reconstruct a `HANDLE` from the `isize` we store (raw `HANDLE` is a
/// non-`Send` pointer; the integer form crosses the thread boundary safely).
#[inline]
fn handle_from(raw: isize) -> HANDLE {
    HANDLE(raw as *mut c_void)
}

/// Open the game process for suspend/resume; `None` on access-denied (usually
/// "not elevated") or if the process vanished.
fn open_game(pid: u32) -> Option<isize> {
    unsafe {
        match OpenProcess(PROCESS_SUSPEND_RESUME, BOOL(0), pid) {
            Ok(h) if !h.is_invalid() => Some(h.0 as isize),
            _ => None,
        }
    }
}

/// Suspend the whole process. Returns true on `STATUS_SUCCESS`.
fn suspend(raw: isize) -> bool {
    unsafe { NtSuspendProcess(handle_from(raw)) == 0 }
}

/// Resume the whole process, retrying like the Python original (the status
/// read races the resume itself, so we call unconditionally rather than trust
/// a "looks running" check). Always closes the handle afterward.
fn resume_and_close(raw: isize) {
    for attempt in 1..=RESUME_MAX_ATTEMPTS {
        let ok = unsafe { NtResumeProcess(handle_from(raw)) == 0 };
        if ok {
            break;
        }
        log_error!("[monitor] NtResumeProcess attempt {attempt}/{RESUME_MAX_ATTEMPTS} failed");
        thread::sleep(RESUME_VERIFY_WAIT);
    }
    unsafe {
        let _ = CloseHandle(handle_from(raw));
    }
}

struct MonitorState {
    /// The watcher loop runs while this is true.
    active: bool,
    /// (pid, open handle as isize) of the suspended game, if any.
    suspended: Option<(u32, isize)>,
    /// When the current suspension began (for the auto-resume timeout).
    suspension_start: Option<Instant>,
    /// Set once resume has been requested; the watcher stops suspending after.
    runoverlay_started: bool,
    /// Unconditional safety net: resume no matter what after this long.
    auto_resume: Duration,
}

impl MonitorState {
    fn new() -> Self {
        Self {
            active: false,
            suspended: None,
            suspension_start: None,
            runoverlay_started: false,
            auto_resume: Duration::from_secs_f64(DEFAULT_AUTO_RESUME_SECS),
        }
    }

    /// Resume the held process (if any) and clear suspension bookkeeping.
    /// Safe to call repeatedly.
    fn resume_held(&mut self) {
        if let Some((pid, raw)) = self.suspended.take() {
            resume_and_close(raw);
            log_info!("[monitor] Resumed game pid={pid}");
        }
        self.suspension_start = None;
    }
}

/// Monitors and controls `League of Legends.exe` suspension. Public methods
/// keep the `&mut self` shape the injection pipeline calls against; the shared
/// state lets the background watcher thread coordinate with them.
pub struct GameMonitor {
    state: Arc<Mutex<MonitorState>>,
    watcher: Option<JoinHandle<()>>,
}

impl GameMonitor {
    pub fn new() -> Self {
        Self { state: Arc::new(Mutex::new(MonitorState::new())), watcher: None }
    }

    /// Override the auto-resume safety timeout (config `monitor_auto_resume_timeout`,
    /// clamped 1..=180s). Takes effect on the next `start()`.
    pub fn set_auto_resume_timeout(&mut self, secs: f64) {
        let clamped = secs.clamp(1.0, 180.0);
        self.state.lock_safe().auto_resume = Duration::from_secs_f64(clamped);
    }

    /// Start watching for the game and suspend it the moment it appears.
    /// Stops any prior watcher first, exactly like the Python original.
    pub fn start(&mut self) {
        self.stop();
        {
            let mut st = self.state.lock_safe();
            st.active = true;
            st.runoverlay_started = false;
            st.suspended = None;
            st.suspension_start = None;
        }
        let state = Arc::clone(&self.state);
        self.watcher = Some(thread::spawn(move || watcher_loop(state)));
        log_info!("[monitor] GameMonitor armed");
    }

    /// Resume the suspended game (called the instant `runoverlay` starts).
    /// Sets `runoverlay_started` unconditionally first — like the Python
    /// original — so the watcher stops suspending even if resume fails.
    pub fn resume(&mut self) {
        {
            let mut st = self.state.lock_safe();
            st.runoverlay_started = true;
            st.resume_held();
            st.active = false;
        }
        self.join_watcher();
    }

    /// Resume only if we actually suspended something, then stop (used when
    /// injection is skipped, e.g. a base skin — never leave the game frozen
    /// waiting for an injection that isn't coming).
    pub fn resume_if_suspended(&mut self) {
        let held = self.state.lock_safe().suspended.is_some();
        if held {
            log_info!("[INJECT] Injection skipped - resuming suspended game");
            self.resume();
        } else {
            self.stop();
        }
    }

    /// True while the watcher is armed.
    pub fn is_active(&self) -> bool {
        self.state.lock_safe().active
    }

    /// Stop the watcher, resuming the game first if it's still suspended
    /// (`InjectionManager` calls this after every injection attempt).
    pub fn stop(&mut self) {
        {
            let mut st = self.state.lock_safe();
            // If a process is still held here, resume() was never reached
            // (injection bailed before runoverlay) — resume it, or the game
            // stays frozen forever.
            st.resume_held();
            st.active = false;
        }
        self.join_watcher();
    }

    fn join_watcher(&mut self) {
        if let Some(h) = self.watcher.take() {
            let _ = h.join();
        }
    }
}

/// Background watcher: rapid startup checks, then steady-state polling, with
/// the unconditional auto-resume timeout enforced every iteration.
fn watcher_loop(state: Arc<Mutex<MonitorState>>) {
    let mut sys = System::new();
    let mut checks_done = 0u32;

    loop {
        {
            let st = state.lock_safe();
            if !st.active || st.runoverlay_started {
                break;
            }
        }

        sys.refresh_processes(ProcessesToUpdate::All, true);
        let game_pid = sys
            .processes()
            .values()
            .find(|p| p.name().to_string_lossy().to_lowercase() == GAME_EXE_NAME)
            .map(|p| p.pid().as_u32());

        let mut interval = if checks_done < RAPID_CHECKS { RAPID_INTERVAL } else { HUNT_INTERVAL };
        checks_done = checks_done.saturating_add(1);

        {
            let mut st = state.lock_safe();
            if !st.active || st.runoverlay_started {
                break;
            }
            match (game_pid, st.suspended.is_some()) {
                (Some(pid), false) => {
                    // Found it and we haven't suspended anything yet.
                    if let Some(raw) = open_game(pid) {
                        if suspend(raw) {
                            st.suspended = Some((pid, raw));
                            st.suspension_start = Some(Instant::now());
                            interval = IDLE_INTERVAL;
                            log_info!("[monitor] Suspended game pid={pid}");
                        } else {
                            unsafe {
                                let _ = CloseHandle(handle_from(raw));
                            }
                            log_error!("[monitor] NtSuspendProcess failed for pid={pid}");
                        }
                    } else {
                        // Access denied usually means not elevated — retrying
                        // forever won't help; surface and stop.
                        log_error!("[monitor] Cannot open game pid={pid} (run as administrator?) - stopping monitor");
                        st.active = false;
                        break;
                    }
                }
                (_, true) => {
                    // Already suspended — just enforce the safety timeout.
                    interval = IDLE_INTERVAL;
                    if let Some(started) = st.suspension_start {
                        if started.elapsed() >= st.auto_resume {
                            log_error!("[monitor] Auto-resume timeout hit - resuming game unconditionally");
                            st.resume_held();
                            st.active = false;
                            break;
                        }
                    }
                }
                (None, false) => {} // still hunting
            }
        }

        thread::sleep(interval);
    }
}

impl Default for GameMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Teardown safety net: never let a dropped monitor leave the game frozen.
impl Drop for GameMonitor {
    fn drop(&mut self) {
        {
            let mut st = self.state.lock_safe();
            st.active = false;
            st.resume_held();
        }
        self.join_watcher();
    }
}

/// Poison-tolerant lock: a panic while suspended must not make every later
/// resume attempt panic too and strand the game.
trait LockSafe<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T>;
}
impl<T> LockSafe<T> for Mutex<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_clears_state_even_with_nothing_suspended() {
        let mut m = GameMonitor::new();
        assert!(!m.is_active());
        m.start();
        assert!(m.is_active());
        m.resume();
        assert!(!m.is_active());
    }

    #[test]
    fn resume_if_suspended_is_noop_when_nothing_suspended() {
        let mut m = GameMonitor::new();
        m.resume_if_suspended();
        assert!(!m.is_active());
    }

    #[test]
    fn auto_resume_timeout_clamps() {
        let mut m = GameMonitor::new();
        m.set_auto_resume_timeout(9999.0);
        assert_eq!(m.state.lock_safe().auto_resume, Duration::from_secs(180));
        m.set_auto_resume_timeout(0.01);
        assert_eq!(m.state.lock_safe().auto_resume, Duration::from_secs(1));
    }
}
