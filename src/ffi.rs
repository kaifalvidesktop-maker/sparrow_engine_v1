//! C ABI for the shell (Go/cgo, C, C++). All functions are panic-safe: a panic never crosses FFI.
//! Strings returned are owned by the handle and valid until the next call on the same handle.
//! Build as cdylib (see PATCHES.md). Header: include/sparrow.h
use crate::SparrowEngine;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null;

pub struct Handle {
    engine: SparrowEngine,
    frame: Vec<u8>,
    err: CString,
    text: CString,
}

fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(default)
}

fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// SAFETY: caller passes a pointer returned by `sparrow_create` (or null).
unsafe fn h<'a>(p: *mut Handle) -> Option<&'a mut Handle> {
    if p.is_null() { None } else { Some(&mut *p) }
}

unsafe fn arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() { None } else { CStr::from_ptr(p).to_str().ok() }
}

fn set_text(hd: &mut Handle, s: &str) -> *const c_char {
    hd.text = cs(s);
    hd.text.as_ptr()
}

fn status(hd: &mut Handle, r: Result<(), String>) -> c_int {
    match r {
        Ok(()) => { hd.err = cs(""); 0 }
        Err(e) => {
            let crashed = hd.engine.is_crashed();
            hd.err = cs(&e);
            if crashed { 2 } else { 1 }
        }
    }
}

#[no_mangle]
pub extern "C" fn sparrow_create() -> *mut Handle {
    guard(std::ptr::null_mut(), || {
        Box::into_raw(Box::new(Handle { engine: SparrowEngine::new(), frame: vec![], err: cs(""), text: cs("") }))
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_destroy(p: *mut Handle) {
    if !p.is_null() { guard((), || drop(Box::from_raw(p))); }
}

/// 0 ok, 1 error (see sparrow_last_error), 2 page crashed (show "Page crashed, reload?", then sparrow_recover).
#[no_mangle]
pub unsafe extern "C" fn sparrow_load_url(p: *mut Handle, url: *const c_char) -> c_int {
    guard(1, || {
        let (Some(hd), Some(u)) = (h(p), arg(url)) else { return 1 };
        let r = hd.engine.load_url(u);
        status(hd, r)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_load_html(p: *mut Handle, html: *const c_char, base: *const c_char) -> c_int {
    guard(1, || {
        let (Some(hd), Some(a), Some(b)) = (h(p), arg(html), arg(base)) else { return 1 };
        let r = hd.engine.load_html(a, b);
        status(hd, r)
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_set_viewport(p: *mut Handle, w: u32, hh: u32) -> c_int {
    guard(1, || {
        let Some(hd) = h(p) else { return 1 };
        let r = hd.engine.resize(w, hh);
        status(hd, r)
    })
}

/// Returns pointer to RGBA pixels (w*h*4) or null on error. Valid until next render/destroy.
#[no_mangle]
pub unsafe extern "C" fn sparrow_render(p: *mut Handle, out_w: *mut u32, out_h: *mut u32) -> *const u8 {
    guard(null(), || {
        let Some(hd) = h(p) else { return null() };
        match hd.engine.render() {
            Ok(px) => {
                let (w, hh) = hd.engine.viewport();
                if !out_w.is_null() { *out_w = w; }
                if !out_h.is_null() { *out_h = hh; }
                hd.frame = px;
                hd.err = cs("");
                hd.frame.as_ptr()
            }
            Err(e) => { hd.err = cs(&e); null() }
        }
    })
}

/// Returns the result string, or null on error (see sparrow_last_error).
#[no_mangle]
pub unsafe extern "C" fn sparrow_eval_js(p: *mut Handle, code: *const c_char) -> *const c_char {
    guard(null(), || {
        let (Some(hd), Some(c)) = (h(p), arg(code)) else { return null() };
        match hd.engine.eval_js(c) {
            Ok(s) => set_text(hd, &s),
            Err(e) => { hd.err = cs(&e); null() }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_last_error(p: *mut Handle) -> *const c_char {
    guard(null(), || h(p).map(|hd| hd.err.as_ptr()).unwrap_or(null()))
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_title(p: *mut Handle) -> *const c_char {
    guard(null(), || { let Some(hd) = h(p) else { return null() }; let t = hd.engine.title().to_string(); set_text(hd, &t) })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_url(p: *mut Handle) -> *const c_char {
    guard(null(), || { let Some(hd) = h(p) else { return null() }; let t = hd.engine.url().to_string(); set_text(hd, &t) })
}

/// console.log/alert lines since last call, joined by '\n'.
#[no_mangle]
pub unsafe extern "C" fn sparrow_take_logs(p: *mut Handle) -> *const c_char {
    guard(null(), || { let Some(hd) = h(p) else { return null() }; let t = hd.engine.take_logs().join("\n"); set_text(hd, &t) })
}

/// URL JS asked to navigate to, or null.
#[no_mangle]
pub unsafe extern "C" fn sparrow_take_navigation(p: *mut Handle) -> *const c_char {
    guard(null(), || {
        let Some(hd) = h(p) else { return null() };
        match hd.engine.take_navigation() { Some(u) => set_text(hd, &u), None => null() }
    })
}

/// Link under viewport point (x, y), or null.
#[no_mangle]
pub unsafe extern "C" fn sparrow_hit_test(p: *mut Handle, x: f32, y: f32) -> *const c_char {
    guard(null(), || {
        let Some(hd) = h(p) else { return null() };
        match hd.engine.hit_test(x, y) { Some(u) => set_text(hd, &u), None => null() }
    })
}

/// Returns new scroll_y (or -1.0 on error).
#[no_mangle]
pub unsafe extern "C" fn sparrow_scroll_by(p: *mut Handle, dy: f32) -> f32 {
    guard(-1.0, || {
        let Some(hd) = h(p) else { return -1.0 };
        match hd.engine.scroll_by(dy) { Ok(v) => v, Err(e) => { hd.err = cs(&e); -1.0 } }
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_fire_event(p: *mut Handle, id: *const c_char, ev: *const c_char) -> c_int {
    guard(1, || {
        let (Some(hd), Some(i), Some(e)) = (h(p), arg(id), arg(ev)) else { return 1 };
        let r = hd.engine.fire_event(i, e).map(|_| ());
        status(hd, r)
    })
}

/// Call every frame with elapsed ms. Returns callbacks run, or -1 on error.
#[no_mangle]
pub unsafe extern "C" fn sparrow_run_timers(p: *mut Handle, ms: u32) -> c_int {
    guard(-1, || {
        let Some(hd) = h(p) else { return -1 };
        match hd.engine.run_timers(ms) { Ok(n) => n as c_int, Err(e) => { hd.err = cs(&e); -1 } }
    })
}

/// "x,y,w,h;x,y,w,h;..." in document coordinates.
#[no_mangle]
pub unsafe extern "C" fn sparrow_find(p: *mut Handle, q: *const c_char) -> *const c_char {
    guard(null(), || {
        let (Some(hd), Some(q)) = (h(p), arg(q)) else { return null() };
        match hd.engine.find_in_page(q) {
            Ok(v) => {
                let s: Vec<String> = v.iter().map(|r| format!("{},{},{},{}", r.0, r.1, r.2, r.3)).collect();
                set_text(hd, &s.join(";"))
            }
            Err(e) => { hd.err = cs(&e); null() }
        }
    })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_page_text(p: *mut Handle) -> *const c_char {
    guard(null(), || { let Some(hd) = h(p) else { return null() }; let t = hd.engine.page_text(); set_text(hd, &t) })
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_is_crashed(p: *mut Handle) -> c_int {
    guard(1, || h(p).map(|hd| hd.engine.is_crashed() as c_int).unwrap_or(1))
}

#[no_mangle]
pub unsafe extern "C" fn sparrow_recover(p: *mut Handle) {
    guard((), || { if let Some(hd) = h(p) { hd.engine.recover(); hd.err = cs(""); } })
}
