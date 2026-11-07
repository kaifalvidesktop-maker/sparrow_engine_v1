//! Runs the engine on its OWN thread with a big stack and talks to it through messages.
//! - `EngineHost::call(closure)` : run any code on the engine thread (panics are caught -> "Page crashed, reload?").
//! - `Cmd` / `Reply` + `execute()` : the full engine API as plain data, with a binary codec (`to_bytes/from_bytes`),
//!   so the same protocol can later run over a pipe to a child process (real crash isolation).
//! Limits: a big stack makes stack overflow far less likely but cannot make it impossible; only a separate
//! process survives a hard overflow / OOM. Panics are fully handled.
use crate::SparrowEngine;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

pub const CRASH_MSG: &str = "Page crashed, reload?";
pub const DEFAULT_STACK: usize = 256 * 1024 * 1024; // address space reserve; pages are committed lazily

// ------------------------------------------------------------------ protocol
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    LoadUrl(String), LoadHtml { html: String, base: String }, Reload, Recover, Render, EvalJs(String),
    SetViewport(u32, u32), ScrollBy(f32), HitTest(f32, f32), TakeLogs, TakeNavigation, RunTimers(u32),
    MediaTick, MediaClick(f32, f32), MediaKey(String), FireEvent { id: String, event: String },
    Find(String), PageText, Info,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Ok, Err(String), Crashed, Frame { w: u32, h: u32, rgba: Vec<u8> }, Text(String), OptText(Option<String>),
    Lines(Vec<String>), Num(f32), Count(usize), Bool(bool), Rects(Vec<(f32, f32, f32, f32)>),
    Info { url: String, title: String, w: u32, h: u32, crashed: bool },
}

/// Execute one command on an engine. Never panics outward (engine methods are already guarded).
pub fn execute(e: &mut SparrowEngine, c: Cmd) -> Reply {
    fn unit(e: &SparrowEngine, r: Result<(), String>) -> Reply { match r { Ok(()) => Reply::Ok, Err(m) => fail(e, m) } }
    fn fail(e: &SparrowEngine, m: String) -> Reply { if e.is_crashed() { Reply::Crashed } else { Reply::Err(m) } }
    match c {
        Cmd::LoadUrl(u) => { let r = e.load_url(&u); unit(e, r) }
        Cmd::LoadHtml { html, base } => { let r = e.load_html(&html, &base); unit(e, r) }
        Cmd::Reload => { let r = e.reload(); unit(e, r) }
        Cmd::Recover => { e.recover(); Reply::Ok }
        Cmd::Render => match e.render() {
            Ok(rgba) => { let (w, h) = e.viewport(); Reply::Frame { w, h, rgba } }
            Err(m) => fail(e, m),
        },
        Cmd::EvalJs(s) => match e.eval_js(&s) { Ok(v) => Reply::Text(v), Err(m) => fail(e, m) },
        Cmd::SetViewport(w, h) => { let r = e.resize(w, h); unit(e, r) }
        Cmd::ScrollBy(dy) => match e.scroll_by(dy) { Ok(v) => Reply::Num(v), Err(m) => fail(e, m) },
        Cmd::HitTest(x, y) => Reply::OptText(e.hit_test(x, y)),
        Cmd::TakeLogs => Reply::Lines(e.take_logs()),
        Cmd::TakeNavigation => Reply::OptText(e.take_navigation()),
        Cmd::RunTimers(ms) => match e.run_timers(ms) { Ok(n) => Reply::Count(n), Err(m) => fail(e, m) },
        Cmd::MediaTick => match e.media_tick() { Ok(b) => Reply::Bool(b), Err(m) => fail(e, m) },
        Cmd::MediaClick(x, y) => Reply::Bool(e.media_click(x, y)),
        Cmd::MediaKey(k) => Reply::Bool(e.media_key(&k)),
        Cmd::FireEvent { id, event } => match e.fire_event(&id, &event) { Ok(b) => Reply::Bool(b), Err(m) => fail(e, m) },
        Cmd::Find(q) => match e.find_in_page(&q) { Ok(v) => Reply::Rects(v), Err(m) => fail(e, m) },
        Cmd::PageText => Reply::Text(e.page_text()),
        Cmd::Info => { let (w, h) = e.viewport(); Reply::Info { url: e.url().to_string(), title: e.title().to_string(), w, h, crashed: e.is_crashed() } }
    }
}

// ------------------------------------------------------------------ binary codec (no deps, no panics)
struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) { self.0.push(v) }
    fn u32(&mut self, v: u32) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn f32(&mut self, v: f32) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn bytes(&mut self, b: &[u8]) { self.u32(b.len() as u32); self.0.extend_from_slice(b) }
    fn str(&mut self, s: &str) { self.bytes(s.as_bytes()) }
}
struct R<'a> { b: &'a [u8], i: usize }
impl<'a> R<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.i.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("truncated message")?;
        let s = &self.b[self.i..end]; self.i = end; Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> { Ok(self.take(1)?[0]) }
    fn u32(&mut self) -> Result<u32, String> { let s = self.take(4)?; Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]])) }
    fn f32(&mut self) -> Result<f32, String> { let s = self.take(4)?; Ok(f32::from_le_bytes([s[0], s[1], s[2], s[3]])) }
    fn bytes(&mut self) -> Result<Vec<u8>, String> { let n = self.u32()? as usize; if n > 512 << 20 { return Err("message too large".into()); } Ok(self.take(n)?.to_vec()) }
    fn str(&mut self) -> Result<String, String> { String::from_utf8(self.bytes()?).map_err(|_| "invalid utf-8".to_string()) }
    fn done(&self) -> Result<(), String> { if self.i == self.b.len() { Ok(()) } else { Err("trailing bytes".into()) } }
}

impl Cmd {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(vec![]);
        match self {
            Cmd::LoadUrl(s) => { w.u8(1); w.str(s) }
            Cmd::LoadHtml { html, base } => { w.u8(2); w.str(html); w.str(base) }
            Cmd::Reload => w.u8(3), Cmd::Recover => w.u8(4), Cmd::Render => w.u8(5),
            Cmd::EvalJs(s) => { w.u8(6); w.str(s) }
            Cmd::SetViewport(a, b) => { w.u8(7); w.u32(*a); w.u32(*b) }
            Cmd::ScrollBy(d) => { w.u8(8); w.f32(*d) }
            Cmd::HitTest(x, y) => { w.u8(9); w.f32(*x); w.f32(*y) }
            Cmd::TakeLogs => w.u8(10), Cmd::TakeNavigation => w.u8(11),
            Cmd::RunTimers(n) => { w.u8(12); w.u32(*n) }
            Cmd::MediaTick => w.u8(13),
            Cmd::MediaClick(x, y) => { w.u8(14); w.f32(*x); w.f32(*y) }
            Cmd::MediaKey(k) => { w.u8(15); w.str(k) }
            Cmd::FireEvent { id, event } => { w.u8(16); w.str(id); w.str(event) }
            Cmd::Find(q) => { w.u8(17); w.str(q) }
            Cmd::PageText => w.u8(18), Cmd::Info => w.u8(19),
        }
        w.0
    }
    pub fn from_bytes(b: &[u8]) -> Result<Cmd, String> {
        let mut r = R { b, i: 0 };
        let c = match r.u8()? {
            1 => Cmd::LoadUrl(r.str()?), 2 => Cmd::LoadHtml { html: r.str()?, base: r.str()? }, 3 => Cmd::Reload, 4 => Cmd::Recover, 5 => Cmd::Render,
            6 => Cmd::EvalJs(r.str()?), 7 => Cmd::SetViewport(r.u32()?, r.u32()?), 8 => Cmd::ScrollBy(r.f32()?), 9 => Cmd::HitTest(r.f32()?, r.f32()?),
            10 => Cmd::TakeLogs, 11 => Cmd::TakeNavigation, 12 => Cmd::RunTimers(r.u32()?), 13 => Cmd::MediaTick,
            14 => Cmd::MediaClick(r.f32()?, r.f32()?), 15 => Cmd::MediaKey(r.str()?), 16 => Cmd::FireEvent { id: r.str()?, event: r.str()? },
            17 => Cmd::Find(r.str()?), 18 => Cmd::PageText, 19 => Cmd::Info,
            t => return Err(format!("unknown command tag {t}")),
        };
        r.done()?;
        Ok(c)
    }
}

impl Reply {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(vec![]);
        match self {
            Reply::Ok => w.u8(1), Reply::Crashed => w.u8(3),
            Reply::Err(s) => { w.u8(2); w.str(s) }
            Reply::Frame { w: fw, h, rgba } => { w.u8(4); w.u32(*fw); w.u32(*h); w.bytes(rgba) }
            Reply::Text(s) => { w.u8(5); w.str(s) }
            Reply::OptText(o) => { w.u8(6); match o { Some(s) => { w.u8(1); w.str(s) } None => w.u8(0) } }
            Reply::Lines(v) => { w.u8(7); w.u32(v.len() as u32); for s in v { w.str(s) } }
            Reply::Num(f) => { w.u8(8); w.f32(*f) }
            Reply::Count(n) => { w.u8(9); w.u32(*n as u32) }
            Reply::Bool(b) => { w.u8(10); w.u8(*b as u8) }
            Reply::Rects(v) => { w.u8(11); w.u32(v.len() as u32); for r in v { w.f32(r.0); w.f32(r.1); w.f32(r.2); w.f32(r.3) } }
            Reply::Info { url, title, w: iw, h, crashed } => { w.u8(12); w.str(url); w.str(title); w.u32(*iw); w.u32(*h); w.u8(*crashed as u8) }
        }
        w.0
    }
    pub fn from_bytes(b: &[u8]) -> Result<Reply, String> {
        let mut r = R { b, i: 0 };
        let v = match r.u8()? {
            1 => Reply::Ok, 2 => Reply::Err(r.str()?), 3 => Reply::Crashed,
            4 => Reply::Frame { w: r.u32()?, h: r.u32()?, rgba: r.bytes()? },
            5 => Reply::Text(r.str()?),
            6 => Reply::OptText(if r.u8()? == 1 { Some(r.str()?) } else { None }),
            7 => { let n = r.u32()? as usize; let mut v = vec![]; for _ in 0..n.min(1 << 20) { v.push(r.str()?); } Reply::Lines(v) }
            8 => Reply::Num(r.f32()?), 9 => Reply::Count(r.u32()? as usize), 10 => Reply::Bool(r.u8()? != 0),
            11 => { let n = r.u32()? as usize; let mut v = vec![]; for _ in 0..n.min(1 << 20) { v.push((r.f32()?, r.f32()?, r.f32()?, r.f32()?)); } Reply::Rects(v) }
            12 => Reply::Info { url: r.str()?, title: r.str()?, w: r.u32()?, h: r.u32()?, crashed: r.u8()? != 0 },
            t => return Err(format!("unknown reply tag {t}")),
        };
        r.done()?;
        Ok(v)
    }
}

// ------------------------------------------------------------------ the host thread
type Job = Box<dyn FnOnce(&mut SparrowEngine) + Send>;

pub struct EngineHost { tx: Option<Sender<Job>>, handle: Option<JoinHandle<()>> }

impl EngineHost {
    pub fn spawn() -> Result<Self, String> { Self::spawn_with_stack(DEFAULT_STACK) }

    /// Tries `stack` bytes, then smaller stacks if the OS refuses (32-bit, low memory).
    pub fn spawn_with_stack(stack: usize) -> Result<Self, String> {
        let mut last = String::new();
        for size in [stack, stack / 4, 16 * 1024 * 1024] {
            let (tx, rx) = channel::<Job>();
            match std::thread::Builder::new().name("sparrow-engine".into()).stack_size(size).spawn(move || {
                // the engine is created ON this thread and never leaves it (boa Context is !Send)
                let mut engine = SparrowEngine::new();
                while let Ok(job) = rx.recv() { job(&mut engine); }
            }) {
                Ok(h) => return Ok(EngineHost { tx: Some(tx), handle: Some(h) }),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("cannot start engine thread: {last}"))
    }

    pub fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut SparrowEngine) -> R + Send + 'static) -> Result<R, String> { self.call_timeout(f, None) }

    /// With a timeout the call returns "engine busy" but the engine keeps working on the job.
    pub fn call_timeout<R: Send + 'static>(&self, f: impl FnOnce(&mut SparrowEngine) -> R + Send + 'static, timeout: Option<Duration>) -> Result<R, String> {
        let tx = self.tx.as_ref().ok_or("engine stopped")?;
        let (rtx, rrx) = channel::<Result<R, String>>();
        let job: Job = Box::new(move |e| {
            let r = match catch_unwind(AssertUnwindSafe(|| f(e))) {
                Ok(v) => Ok(v),
                Err(_) => { let _ = catch_unwind(AssertUnwindSafe(|| e.recover())); Err(CRASH_MSG.to_string()) }
            };
            let _ = rtx.send(r);
        });
        tx.send(job).map_err(|_| "engine thread is gone".to_string())?;
        match timeout {
            None => rrx.recv().map_err(|_| "engine thread is gone".to_string())?,
            Some(d) => match rrx.recv_timeout(d) {
                Ok(r) => r,
                Err(RecvTimeoutError::Timeout) => Err("engine busy (timeout)".into()),
                Err(RecvTimeoutError::Disconnected) => Err("engine thread is gone".into()),
            },
        }
    }

    /// Run one protocol command.
    pub fn exec(&self, c: Cmd) -> Reply {
        match self.call(move |e| execute(e, c)) {
            Ok(r) => r,
            Err(m) if m == CRASH_MSG => Reply::Crashed,
            Err(m) => Reply::Err(m),
        }
    }

    pub fn is_alive(&self) -> bool { self.handle.as_ref().map_or(false, |h| !h.is_finished()) }
}

impl Drop for EngineHost {
    fn drop(&mut self) { self.tx = None; if let Some(h) = self.handle.take() { let _ = h.join(); } }
}


// ------------------------------------------------------------------ drop-in facade
use std::cell::Cell;

/// Same method names/signatures as `SparrowEngine`, but every call runs on the engine thread.
/// `ffi.rs` only needs `SparrowEngine` -> `RemoteEngine` (3 lines) to get big-stack + panic isolation.
pub struct RemoteEngine { host: Option<EngineHost>, url: String, title: String, vp: (u32, u32), crashed: Cell<bool> }

impl RemoteEngine {
    pub fn new() -> Self {
        RemoteEngine { host: EngineHost::spawn().ok(), url: "about:blank".into(), title: String::new(), vp: (1024, 768), crashed: Cell::new(false) }
    }
    fn exec(&self, c: Cmd) -> Reply {
        let r = match &self.host { Some(h) => h.exec(c), None => Reply::Err("engine failed to start".into()) };
        if matches!(r, Reply::Crashed) { self.crashed.set(true); }
        r
    }
    fn unit(r: Reply) -> Result<(), String> {
        match r { Reply::Ok => Ok(()), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }
    /// Sync state, but never lose a crash that the host already reported (host recovers the engine after a panic).
    fn fin<T>(&mut self, r: Result<T, String>) -> Result<T, String> {
        self.sync();
        if matches!(&r, Err(m) if m == CRASH_MSG) { self.crashed.set(true); }
        r
    }
    fn sync(&mut self) {
        if let Reply::Info { url, title, w, h, crashed } = self.exec(Cmd::Info) { self.url = url; self.title = title; self.vp = (w, h); self.crashed.set(crashed); }
    }
    fn boolean(r: Reply) -> Result<bool, String> {
        match r { Reply::Bool(b) => Ok(b), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }

    pub fn is_crashed(&self) -> bool { self.crashed.get() }
    pub fn url(&self) -> &str { &self.url }
    pub fn title(&self) -> &str { &self.title }
    pub fn viewport(&self) -> (u32, u32) { self.vp }
    pub fn recover(&mut self) { let _ = self.exec(Cmd::Recover); self.crashed.set(false); self.sync(); }
    pub fn load_url(&mut self, u: &str) -> Result<(), String> { let r = Self::unit(self.exec(Cmd::LoadUrl(u.into()))); self.fin(r) }
    pub fn load_html(&mut self, html: &str, base: &str) -> Result<(), String> { let r = Self::unit(self.exec(Cmd::LoadHtml { html: html.into(), base: base.into() })); self.fin(r) }
    pub fn reload(&mut self) -> Result<(), String> { let r = Self::unit(self.exec(Cmd::Reload)); self.fin(r) }
    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String> { let r = Self::unit(self.exec(Cmd::SetViewport(w, h))); self.fin(r) }
    pub fn render(&mut self) -> Result<Vec<u8>, String> {
        match self.exec(Cmd::Render) {
            Reply::Frame { w, h, rgba } => { self.vp = (w, h); Ok(rgba) }
            Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()),
        }
    }
    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        match self.exec(Cmd::EvalJs(code.into())) { Reply::Text(t) => Ok(t), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }
    pub fn take_logs(&mut self) -> Vec<String> { match self.exec(Cmd::TakeLogs) { Reply::Lines(v) => v, _ => vec![] } }
    pub fn take_navigation(&mut self) -> Option<String> { match self.exec(Cmd::TakeNavigation) { Reply::OptText(o) => o, _ => None } }
    pub fn hit_test(&self, x: f32, y: f32) -> Option<String> { match self.exec(Cmd::HitTest(x, y)) { Reply::OptText(o) => o, _ => None } }
    pub fn page_text(&self) -> String { match self.exec(Cmd::PageText) { Reply::Text(t) => t, _ => String::new() } }
    pub fn scroll_by(&mut self, dy: f32) -> Result<f32, String> {
        match self.exec(Cmd::ScrollBy(dy)) { Reply::Num(v) => Ok(v), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }
    pub fn fire_event(&mut self, id: &str, ev: &str) -> Result<bool, String> { let r = Self::boolean(self.exec(Cmd::FireEvent { id: id.into(), event: ev.into() })); self.fin(r) }
    pub fn run_timers(&mut self, ms: u32) -> Result<usize, String> {
        match self.exec(Cmd::RunTimers(ms)) { Reply::Count(n) => Ok(n), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }
    pub fn find_in_page(&mut self, q: &str) -> Result<Vec<(f32, f32, f32, f32)>, String> {
        match self.exec(Cmd::Find(q.into())) { Reply::Rects(v) => Ok(v), Reply::Err(m) => Err(m), Reply::Crashed => Err(CRASH_MSG.into()), _ => Err("protocol error".into()) }
    }
    pub fn media_tick(&mut self) -> Result<bool, String> { Self::boolean(self.exec(Cmd::MediaTick)) }
    pub fn media_click(&mut self, x: f32, y: f32) -> bool { matches!(self.exec(Cmd::MediaClick(x, y)), Reply::Bool(true)) }
    pub fn media_key(&mut self, k: &str) -> bool { matches!(self.exec(Cmd::MediaKey(k.into())), Reply::Bool(true)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_roundtrip_all_commands_and_replies() {
        let cmds = vec![
            Cmd::LoadUrl("https://x.org/বাংলা".into()), Cmd::LoadHtml { html: "<p>é</p>".into(), base: "b".into() }, Cmd::Reload, Cmd::Recover, Cmd::Render,
            Cmd::EvalJs("1+1".into()), Cmd::SetViewport(800, 600), Cmd::ScrollBy(-12.5), Cmd::HitTest(1.0, 2.0), Cmd::TakeLogs, Cmd::TakeNavigation,
            Cmd::RunTimers(16), Cmd::MediaTick, Cmd::MediaClick(3.0, 4.0), Cmd::MediaKey(" ".into()), Cmd::FireEvent { id: "a".into(), event: "click".into() },
            Cmd::Find("q".into()), Cmd::PageText, Cmd::Info,
        ];
        for c in cmds { assert_eq!(Cmd::from_bytes(&c.to_bytes()).expect("decode"), c); }
        let replies = vec![
            Reply::Ok, Reply::Err("e".into()), Reply::Crashed, Reply::Frame { w: 2, h: 1, rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] }, Reply::Text("t".into()),
            Reply::OptText(Some("s".into())), Reply::OptText(None), Reply::Lines(vec!["a".into(), "b".into()]), Reply::Num(1.5), Reply::Count(7), Reply::Bool(true),
            Reply::Rects(vec![(1.0, 2.0, 3.0, 4.0)]), Reply::Info { url: "u".into(), title: "t".into(), w: 1, h: 2, crashed: true },
        ];
        for r in replies { assert_eq!(Reply::from_bytes(&r.to_bytes()).expect("decode"), r); }
    }

    #[test]
    fn bad_input_is_an_error_never_a_panic() {
        assert!(Cmd::from_bytes(&[]).is_err());
        assert!(Cmd::from_bytes(&[99]).is_err());
        assert!(Cmd::from_bytes(&[1, 255, 255, 255, 255]).is_err()); // huge length
        assert!(Cmd::from_bytes(&[1, 2, 0, 0, 0, b'a']).is_err());   // truncated
        assert!(Cmd::from_bytes(&[3, 0]).is_err());                   // trailing byte
        assert!(Reply::from_bytes(&[7, 255, 255, 255, 255]).is_err());
        let ok = Cmd::LoadUrl("abc".into()).to_bytes();
        for cut in 0..ok.len() { assert!(Cmd::from_bytes(&ok[..cut]).is_err()); }
    }

    #[test]
    fn host_runs_commands_on_engine_thread() {
        let h = EngineHost::spawn_with_stack(32 * 1024 * 1024).expect("spawn");
        assert_eq!(h.exec(Cmd::LoadUrl("https://ok.test/".into())), Reply::Ok);
        match h.exec(Cmd::Info) { Reply::Info { url, .. } => assert_eq!(url, "https://ok.test/"), r => panic!("{r:?}") }
        assert_eq!(h.exec(Cmd::LoadUrl("boom".into())), Reply::Err("bad url".into()));
        assert!(h.is_alive());
    }

    #[test]
    fn panic_inside_job_is_contained_and_engine_recovers() {
        let h = EngineHost::spawn_with_stack(32 * 1024 * 1024).expect("spawn");
        let r: Result<(), String> = h.call(|_e| { panic!("simulated engine bug") });
        assert_eq!(r, Err(CRASH_MSG.to_string()));
        assert!(h.is_alive());
        assert_eq!(h.exec(Cmd::LoadUrl("https://after.test/".into())), Reply::Ok); // still serving
        assert_eq!(h.exec(Cmd::LoadUrl("panic".into())), Reply::Crashed);          // engine-level crash mapped to Crashed
    }

    #[test]
    fn big_stack_survives_deep_recursion_that_small_stacks_cannot() {
        fn deep(n: u64) -> u64 { let pad = [n as u8; 256]; if n == 0 { pad[0] as u64 } else { deep(n - 1) + std::hint::black_box(pad[1]) as u64 } }
        let h = EngineHost::spawn_with_stack(256 * 1024 * 1024).expect("spawn");
        let v = h.call(|_e| deep(300_000)).expect("deep recursion ran"); // ~100 MB of stack
        assert_eq!(v, (1..=300_000u64).map(|n| (n as u8) as u64).sum::<u64>()); // finished without overflowing
    }

    #[test]
    fn timeout_reports_busy_and_host_survives() {
        let h = EngineHost::spawn_with_stack(32 * 1024 * 1024).expect("spawn");
        let r = h.call_timeout(|_e| std::thread::sleep(Duration::from_millis(300)), Some(Duration::from_millis(20)));
        assert_eq!(r, Err("engine busy (timeout)".to_string()));
        assert_eq!(h.call(|_e| 7).expect("later call"), 7);
    }

    #[test]
    fn drop_joins_thread() {
        let h = EngineHost::spawn_with_stack(16 * 1024 * 1024).expect("spawn");
        drop(h); // must not hang
    }

    #[test]
    fn remote_engine_is_a_drop_in_with_crash_state() {
        let mut r = RemoteEngine::new();
        r.load_url("https://a.test/x").expect("load");
        assert_eq!(r.url(), "https://a.test/x");
        assert_eq!(r.viewport(), (1024, 768));
        r.resize(320, 200).expect("resize");
        assert_eq!(r.render().expect("frame").len(), 320 * 200 * 4);
        assert_eq!(r.load_url("panic"), Err(CRASH_MSG.to_string()));
        assert!(r.is_crashed());
        r.recover();
        assert!(!r.is_crashed());
        assert_eq!(r.eval_js("1+1").expect("js"), "1+1");
        assert_eq!(r.scroll_by(4.0).expect("scroll"), 4.0);
        assert_eq!(r.hit_test(1.0, 1.0), None);
        assert!(r.take_logs().is_empty());
    }
}
