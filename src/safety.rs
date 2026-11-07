//! Crash/hang protection helpers (std only).
//! NOTE: a real stack overflow or OOM still kills the process; big stacks make it rare,
//! process isolation (later) removes it.
use std::collections::VecDeque;
use std::panic::{self, catch_unwind, UnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

const LOG_CAP: usize = 200;
const MAX_ABANDONED: usize = 4;
static LOG: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
static ABANDONED: AtomicUsize = AtomicUsize::new(0);
static HOOK: Once = Once::new();

fn ring() -> &'static Mutex<VecDeque<String>> { LOG.get_or_init(|| Mutex::new(VecDeque::new())) }

/// RAM-only ring log (never touches disk).
pub fn log(msg: impl Into<String>) {
    if let Ok(mut q) = ring().lock() {
        if q.len() >= LOG_CAP { q.pop_front(); }
        q.push_back(msg.into());
    }
}
pub fn recent_logs() -> Vec<String> { ring().lock().map(|q| q.iter().cloned().collect()).unwrap_or_default() }
pub fn clear_logs() { if let Ok(mut q) = ring().lock() { q.clear(); } }

fn payload_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() { (*s).to_string() }
    else if let Some(s) = p.downcast_ref::<String>() { s.clone() }
    else { "unknown panic".into() }
}

/// Panics are written to the RAM log instead of stderr/disk.
pub fn install_panic_hook() {
    HOOK.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            log(format!("panic at {loc}: {}", payload_text(info.payload())));
        }));
    });
}

/// Run `f`, turning a panic into `Err`.
pub fn run_guarded<T>(label: &str, f: impl FnOnce() -> T + UnwindSafe) -> Result<T, String> {
    catch_unwind(f).map_err(|p| {
        let m = format!("{label}: {}", payload_text(&*p));
        log(m.clone());
        m
    })
}

/// Run `f` on a fresh thread with a large stack (deep DOM/CSS recursion).
pub fn run_big_stack<T: Send + 'static>(stack_mb: usize, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let h = std::thread::Builder::new()
        .name("sparrow-worker".into())
        .stack_size(stack_mb.clamp(1, 512) * 1024 * 1024)
        .spawn(f)
        .map_err(|e| format!("spawn failed: {e}"))?;
    h.join().map_err(|p| {
        let m = format!("worker panicked: {}", payload_text(&*p));
        log(m.clone());
        m
    })
}

/// Like `run_big_stack` but gives up after `ms`. A stuck thread cannot be killed in safe Rust:
/// it is abandoned; after MAX_ABANDONED stuck threads new jobs are refused (no unbounded leak).
pub fn run_with_timeout<T: Send + 'static>(ms: u64, stack_mb: usize, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    if ABANDONED.load(Ordering::SeqCst) >= MAX_ABANDONED {
        return Err("too many stuck workers; restart the engine".into());
    }
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("sparrow-timed".into())
        .stack_size(stack_mb.clamp(1, 512) * 1024 * 1024)
        .spawn(move || { let r = catch_unwind(panic::AssertUnwindSafe(f)); let _ = tx.send(r); })
        .map_err(|e| format!("spawn failed: {e}"))?;
    match rx.recv_timeout(Duration::from_millis(ms)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(p)) => { let m = format!("worker panicked: {}", payload_text(&*p)); log(m.clone()); Err(m) }
        Err(_) => { ABANDONED.fetch_add(1, Ordering::SeqCst); log(format!("timeout after {ms} ms")); Err(format!("timeout after {ms} ms")) }
    }
}
pub fn abandoned_workers() -> usize { ABANDONED.load(Ordering::SeqCst) }

/// Cooperative deadline for long Rust loops (layout, parsing): call `expired()` every N iterations.
pub struct Deadline { end: Instant }
impl Deadline {
    pub fn after_ms(ms: u64) -> Self { Deadline { end: Instant::now() + Duration::from_millis(ms) } }
    pub fn expired(&self) -> bool { Instant::now() >= self.end }
    pub fn check(&self, what: &str) -> Result<(), String> {
        if self.expired() { Err(format!("{what}: time budget exceeded")) } else { Ok(()) }
    }
}

/// Hard limits used by the engine (keep in one place).
pub mod limits {
    pub const MAX_DOM_NODES: usize = 200_000;
    pub const MAX_DOM_DEPTH: usize = 512;
    pub const MAX_CSS_BYTES: usize = 2 * 1024 * 1024;
    pub const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024;
    pub const MAX_PIXELS: u64 = 16_000_000; // 4000x4000 RGBA = 64 MB
    pub const MAX_SIDE: u32 = 16_384;
    pub fn viewport_ok(w: u32, h: u32) -> Result<(), String> {
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || (w as u64) * (h as u64) > MAX_PIXELS { Err(format!("bad viewport {w}x{h}")) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn guarded_catches_panic() {
        install_panic_hook();
        let r = run_guarded("t", || { if true { panic!("boom"); } 1 });
        assert!(r.is_err());
        assert!(recent_logs().iter().any(|l| l.contains("boom")));
    }
    #[test] fn big_stack_deep_recursion() {
        fn d(n: u32) -> u32 { if n == 0 { 0 } else { 1 + d(n - 1) } }
        assert_eq!(run_big_stack(64, || d(100_000)).unwrap(), 100_000);
    }
    #[test] fn timeout_returns() {
        let r = run_with_timeout(50, 2, || std::thread::sleep(Duration::from_millis(400)));
        assert!(r.is_err());
        assert!(abandoned_workers() >= 1);
    }
    #[test] fn deadline_and_limits() {
        assert!(Deadline::after_ms(0).expired());
        assert!(limits::viewport_ok(0, 5).is_err());
        assert!(limits::viewport_ok(800, 600).is_ok());
    }
    #[test] fn log_ring_is_bounded() {
        for i in 0..500 { log(format!("x{i}")); }
        assert!(recent_logs().len() <= LOG_CAP);
    }
}
