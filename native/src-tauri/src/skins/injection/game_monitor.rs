//! Game process suspend/resume monitor — INTERFACE STUB (S3). The Python
//! original (`injection\game\game_monitor.py::GameMonitor`) suspends
//! `League of Legends.exe` moments after it launches during champion
//! select, so cslol's overlay can hook in before the game finishes loading
//! assets, then resumes it the instant `runoverlay` starts (see
//! `overlay::mk_run_overlay`).
//!
//! DEFERRED TO THE LEAD (S3.1): the actual suspend/resume FFI
//! (`NtSuspendProcess`/`NtResumeProcess`, undocumented `ntdll.dll` exports)
//! needs a `windows` crate surface not in the currently-declared feature
//! list (`Win32_System_Threading` has `SuspendThread`/`ResumeThread` per
//! *thread*, not the whole-process `Nt*SuspendProcess` pair the Python
//! original used, and
//! the safe `windows` crate doesn't expose `ntdll.dll` exports via a
//! documented module — this likely wants either a manually-declared
//! `#[link(name = "ntdll")] extern "system"` block, or `GetProcAddress`
//! against `ntdll.dll` at runtime). Every method below is a documented
//! no-op/log stub that compiles and satisfies the interface `overlay.rs`
//! and `mod.rs` (`InjectionManager`) already call against; the lead fills in
//! the bodies mechanically per the doc comments.

#![allow(dead_code)] // consumed by S3+ (injector/overlay wiring); FFI bodies land in S3.1

use std::time::Instant;

use crate::skins::slog::log_info;

/// Monitors and controls `League of Legends.exe` process suspension/resume
/// (ported from `GameMonitor`). Field shapes mirror the Python instance
/// attributes 1:1 so the lead's FFI fill-in is mechanical.
pub struct GameMonitor {
    /// True while the background monitor loop should keep running (Python's
    /// `self._monitor_active`).
    monitor_active: bool,
    /// PID of the game process we suspended, if any (Python's
    /// `self._suspended_game_process`, minus the `psutil.Process` wrapper —
    /// S3.1 will likely also want a stored process `HANDLE` here for
    /// `NtResumeProcess`, since a PID alone isn't enough to resume via the
    /// native API without re-opening a handle on every call).
    suspended_pid: Option<u32>,
    /// Wall-clock instant the suspension started, for the auto-resume
    /// safety timeout (Python's `suspension_start_time`, a local variable
    /// inside the `game_monitor()` thread body — hoisted to a field here
    /// since Rust has no closure-captured mutable local shared across polls
    /// the way the Python thread body did).
    suspension_start: Option<Instant>,
    /// Set once `resume()` has been called for this session, so the
    /// monitor loop (S3.1) stops suspending anything further even if it's
    /// still polling (Python's `self._runoverlay_started`).
    runoverlay_started: bool,
}

impl GameMonitor {
    pub fn new() -> Self {
        Self { monitor_active: false, suspended_pid: None, suspension_start: None, runoverlay_started: false }
    }

    /// Start watching for `League of Legends.exe` and suspend it the moment
    /// it appears (ported from `GameMonitor.start`). Stops any existing
    /// monitor first, exactly like the Python original.
    ///
    /// DEFERRED FFI (S3.1) — exact behavior to reproduce from the Python original:
    /// 1. Spawn a background loop (Python: a daemon thread; either a
    ///    `std::thread` or a blocking tokio task is fine — this struct
    ///    doesn't dictate it).
    /// 2. On start, do 10 rapid immediate checks of the process table (via
    ///    `sysinfo`, ~5ms apart) to catch the game as soon as it launches —
    ///    the client can spawn it before this method is even called.
    /// 3. The moment `League of Legends.exe` is found and not already
    ///    suspended, call `NtSuspendProcess` on its handle; record
    ///    `suspended_pid` + `suspension_start = Instant::now()`.
    /// 4. If the process is found already suspended (race with a previous
    ///    session), just track it — don't double-suspend.
    /// 5. After the immediate checks, fall into the steady-state poll loop:
    ///    `PERSISTENT_MONITOR_CHECK_INTERVAL_S` (50ms) while hunting for the
    ///    process, `PERSISTENT_MONITOR_IDLE_INTERVAL_S` (100ms) once
    ///    suspended and just waiting for `runoverlay` to hook in.
    /// 6. Every iteration while suspended, compare
    ///    `suspension_start.elapsed()` against the configured auto-resume
    ///    timeout (`monitor_auto_resume_timeout` config value, clamped
    ///    1.0..=180.0s, default 60s) — this is the UNCONDITIONAL safety net:
    ///    if injection stalls for any reason, resume anyway rather than
    ///    freeze the client's game forever.
    /// 7. `AccessDenied` opening the process (non-admin) must stop the
    ///    monitor and surface a clear "run as administrator" error — don't
    ///    retry forever.
    pub fn start(&mut self) {
        self.stop();
        self.monitor_active = true;
        self.runoverlay_started = false;
        log_info!("[monitor] GameMonitor::start (stub — NtSuspendProcess deferred to S3.1)");
    }

    /// Resume the suspended game (called the instant `runoverlay` starts —
    /// see `overlay::mk_run_overlay`). Ported from `GameMonitor.resume_game`.
    /// Sets the "runoverlay started" flag UNCONDITIONALLY first, exactly
    /// like the Python original, so the monitor loop stops suspending
    /// anything even if the resume itself fails below.
    ///
    /// DEFERRED FFI (S3.1): call `NtResumeProcess` on the suspended handle,
    /// retrying up to `GAME_RESUME_MAX_ATTEMPTS` (3) with
    /// `GAME_RESUME_VERIFICATION_WAIT_S` (0.1s) between attempts — the
    /// Python original always calls resume even when the reported status already looks
    /// "running" (the status check races the resume itself, so it's safer
    /// to call unconditionally than to trust the status read). Must clear
    /// `suspended_pid`/`suspension_start` and stop the monitor loop
    /// afterwards regardless of whether the resume actually verified —
    /// this is the UNCONDITIONAL auto-resume safety behavior: never leave
    /// the struct believing a process is still suspended once resume has
    /// been attempted.
    pub fn resume(&mut self) {
        self.runoverlay_started = true;
        log_info!("[monitor] GameMonitor::resume (stub — NtResumeProcess deferred to S3.1)");
        self.suspended_pid = None;
        self.suspension_start = None;
        self.monitor_active = false;
    }

    /// Resume the game only if the monitor actually suspended it, then stop
    /// the monitor (ported from `GameMonitor.resume_if_suspended` — used
    /// when injection is skipped, e.g. a base skin was selected, so we
    /// never leave the game frozen waiting for an injection that isn't
    /// coming).
    pub fn resume_if_suspended(&mut self) {
        if self.suspended_pid.is_some() {
            log_info!("[INJECT] Injection skipped - resuming suspended game");
            self.resume();
        }
        self.stop();
    }

    /// True while the monitor is armed/suspending (ported from
    /// `GameMonitor.is_active`).
    pub fn is_active(&self) -> bool {
        self.monitor_active
    }

    /// Stop the monitor, resuming the game first if it's still suspended
    /// (ported from `GameMonitor.stop`). `InjectionManager::_stop_monitor`
    /// calls this directly (not just through `resume()`) after every
    /// injection attempt, successful or not.
    ///
    /// DEFERRED FFI (S3.1): if `suspended_pid` is still set here, that means
    /// `resume()` was never called for this session (e.g. injection bailed
    /// out before reaching `runoverlay`) — call `NtResumeProcess` here too
    /// before clearing state, or the game stays frozen forever.
    pub fn stop(&mut self) {
        if self.monitor_active && self.suspended_pid.is_some() {
            log_info!("[monitor] Stopping with a still-suspended PID (stub — NtResumeProcess deferred to S3.1)");
        }
        self.monitor_active = false;
        self.suspended_pid = None;
        self.suspension_start = None;
    }
}

impl Default for GameMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_always_clears_suspended_state_even_with_nothing_suspended() {
        let mut m = GameMonitor::new();
        assert!(!m.is_active());
        m.start();
        assert!(m.is_active());
        m.resume();
        assert!(!m.is_active());
    }

    #[test]
    fn resume_if_suspended_is_a_noop_when_nothing_was_suspended() {
        let mut m = GameMonitor::new();
        m.resume_if_suspended(); // must not panic
        assert!(!m.is_active());
    }
}
