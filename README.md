# Sparrow Engine

> A Rust browser engine embedded in the **Sparrow** desktop shell (Go + WebView). The shell hands the engine a URL; the engine returns RGBA pixels. **The engine must never crash the shell.**

**বাংলা সারসংক্ষেপ:** এটি Sparrow ব্রাউজারের ভেতরের Rust ইঞ্জিন। শেল URL দেয়, ইঞ্জিন HTML/CSS/JS প্রসেস করে RGBA পিক্সেল ফেরত দেয়। ইঞ্জিন প্যানিক করলেও শেল বেঁচে থাকে এবং "Page crashed, reload?" দেখায়। সব ডেটা RAM-এ থাকে; ডিস্কে শুধু বুকমার্ক ও সেটিংস (ফেজ-৪ ডিজাইন)।

> This README is generated from the source tree (module docs, public API, tests). Hand-written parts: status, architecture, integration, roadmap.

---

## Table of contents

1. [Status](#status)
2. [Design goals](#design-goals)
3. [Architecture](#architecture)
4. [Repository layout](#repository-layout)
5. [Pipeline walkthrough](#pipeline-walkthrough)
6. [Module reference](#module-reference)
7. [C ABI reference](#c-abi-reference)
8. [Shell integration (Go/cgo)](#shell-integration-gocgo)
9. [Phase 4 integration guide](#phase-4-integration-guide)
10. [Storage and privacy model](#storage-and-privacy-model)
11. [Threading and crash model](#threading-and-crash-model)
12. [Security model](#security-model)
13. [Testing](#testing)
14. [Build blockers](#build-blockers)
15. [Not covered yet](#not-covered-yet)
16. [Roadmap](#roadmap)
17. [Engineering rules](#engineering-rules)
18. [Glossary](#glossary)

---

## Status

| Item | State |
|---|---|
| Language | Rust 2021 (`rlib` + `cdylib`) |
| Compiled and tested | **No.** Nothing in this tree has been built by the author of this README. Run `cargo build` first. |
| Rendering | CPU software (tiny-skia); system fonts via fontdb + rustybuzz; bitmap fallback |
| JavaScript | Boa 0.17 (interpreter). V8 is not used. `fetch`/XHR are stubs |
| Network | reqwest (blocking, rustls), RAM cookie jar, RAM cache, HSTS, https-first |
| Storage | Phase 4: plain-text file for bookmarks + settings only; history/session/cache RAM-only |
| Phase 4 modules | Files exist but are **not declared in `lib.rs`** yet (see [Phase 4 integration guide](#phase-4-integration-guide)) |
| GPU | Not used |

Legend used below: **done** = code present, **partial** = present but not wired or incomplete, **missing** = not in tree.

| Area | State | Notes |
|---|---|---|
| HTML parse + DOM arena | done | html5ever, arena `Dom` |
| CSS cascade + extensions | done | `dro_css`, `dro_css_ext` |
| Layout | done | taffy; images, media boxes, `Paint::Image` |
| Paint | done | tiny-skia; bilinear image scaling |
| Text shaping (Bangla/Indic) | partial | needs `unicode-bidi`, `unicode-linebreak` deps |
| JS runtime | partial | Boa; partial DOM shim; no fetch/XHR |
| Media | partial | audio + GIF; no video frame decode |
| Networking | done | cookies, cache, redirects, blocklist |
| Forms (`forms.rs`) | partial | logic done; not called from engine |
| Text input (`input.rs`) | partial | model done; no focus/key wiring |
| HTTP cache (`dro_cache.rs`) | partial | not used by `dro_net` |
| Engine thread (`engine_host.rs`) | partial | not declared in `lib.rs`; ffi/tabs not switched |
| Process isolation | missing | planned `process_host.rs` |
| Same-origin / CORS / CSP | missing | planned `origin.rs` |
| Incremental layout/paint | missing | planned `invalidate.rs`, `raster_cache.rs` |
| Scheduler | missing | planned `scheduler.rs` |

---

## Design goals

Order of priority:

1. Never crash the shell.
2. Correct rendering of real pages.
3. Small binary.
4. Speed.

- One engine owns one page: DOM, styles, layout, JS context, media slots.
- Every public method is panic-guarded; a panic marks the engine crashed and returns `"Page crashed, reload?"`.
- Everything except bookmarks and settings lives in RAM and disappears with the process.
- Strings returned over the C ABI are owned by the handle and valid until the next call.
- No telemetry, no third-party trackers: sub-resources run through a blocklist.

---

## Architecture

```text
Go shell (WebView)
   |  cgo / C ABI  (include/sparrow.h)
   v
ffi.rs  -- Handle { engine, frame, err, text }
   |
   v
engine_host.rs  RemoteEngine -> EngineHost thread (256 MB stack, catch_unwind)
   |   Cmd / Reply  (binary codec, ready for a child process)
   v
sparrow_engine.rs  SparrowEngine (guard(), recover())
   |-- bridge.rs        install_page, JS<->DOM, events, timers, find, page_text
   |-- media_bridge.rs  <audio>/<video> slots, tick, click, key
   |
   |  fetch --> dro_net  (cookies, cache, HSTS, blocklist, redirects)
   |  decode -> dro_charset
   |  parse --> dro_html (html5ever) -> dro_dom (arena)
   |  style --> dro_css + dro_css_ext
   |  layout -> dro_layout (taffy) + dro_font (rustybuzz)
   |  paint --> dro_paint (tiny-skia) + dro_media_ui overlay
   |  script -> js_engine -> dro_js (Boa) + dom_bootstrap.js + dro_dom_js
   v
RGBA frame (viewport w * h * 4)
```

### Data flow of one navigation

1. `load_url` trims input; `same_doc` checks for a fragment-only jump.
2. `fetch_page` calls `dro_net::fetch`, decides the MIME type (declared, else sniffed).
3. HTML is decoded with `dro_charset::decode`; text and JSON are wrapped in `<pre>`; images in `<img>`.
4. `install` sets the page URL for the net layer and calls `bridge::install_page`.
5. `install_page` parses HTML, collects CSS/image/script URLs, fetches them in parallel (`par_map`), builds styles.
6. Scripts run in order; DOM changes made by JS are applied back (`apply_js`).
7. `setup_media` creates a slot per `<audio>`/`<video>`.
8. `render` computes layout if missing, paints, overlays media controls, returns RGBA.

---

## Repository layout

| Path | Lines | Role |
|---|---:|---|
| `CHANGES.md` | 32 |  |
| `Cargo.toml` | 31 |  |
| `PATCHES.md` | 67 |  |
| `PATCHES_P3.md` | 89 |  |
| `PATCHES_P4.md` | 35 |  |
| `README.md` | 633 |  |
| `check.bat` | 9 |  |
| `check.sh` | 5 |  |
| `docs/shell_integration.md` | 69 |  |
| `include/sparrow.h` | 39 |  |
| `phases.md` | 96 |  |
| `prompt_for_html.txt` | 41 |  |
| `src/browser/bookmark.rs` | 30 |  |
| `src/browser/history.rs` | 73 | History is RAM-only by design: nothing is written to disk, everything is gone on exit. |
| `src/browser/mod.rs` | 6 |  |
| `src/browser/omnibox.rs` | 14 |  |
| `src/browser/session.rs` | 51 |  |
| `src/browser/settings.rs` | 34 |  |
| `src/browser/store.rs` | 130 | Tiny persistent store for the ONLY data that lives on disk: bookmarks and settings. |
| `src/browser/tabs.rs` | 73 |  |
| `src/dom_bootstrap.js` | 562 |  |
| `src/dro_blocklist.rs` | 124 | Tracker / ad blocking for sub-resources (scripts, images, CSS, media). Top-level navigation is never blocked. |
| `src/dro_cache.rs` | 187 | RAM-only HTTP cache (never written to disk): Cache-Control max-age / no-store / no-cache / immutable, |
| `src/dro_charset.rs` | 91 | Charset detection + decoding without extra crates. |
| `src/dro_css.rs` | 232 |  |
| `src/dro_css_ext.rs` | 312 | CSS pre-processor: turns modern CSS into the subset `dro_css::parse` understands. |
| `src/dro_dom.rs` | 83 |  |
| `src/dro_dom_js.rs` | 110 | Bridge between our Rust DOM and the JS DOM layer (`dom_bootstrap.js`). |
| `src/dro_font.rs` | 615 | dro_font v2 — real text for Sparrow: system fonts (fontdb), OpenType shaping (rustybuzz, Bangla conjuncts), |
| `src/dro_html.rs` | 118 |  |
| `src/dro_js.rs` | 44 |  |
| `src/dro_layout.rs` | 563 | Sparrow layout: DOM + computed styles -> positioned paint items (via taffy). |
| `src/dro_media.rs` | 336 | Pure-Rust media player (no FFmpeg, no C). |
| `src/dro_media_ui.rs` | 120 | Chromium-style media control bar: draw + hit-test. Coordinates are relative to the media box. |
| `src/dro_net.rs` | 727 | dro_net.rs — Sparrow network stack. RAM-only: cookies, cache, HSTS never touch disk. |
| `src/dro_paint.rs` | 190 | Sparrow paint: layout items -> RGBA pixels (tiny-skia + built-in 5x7 bitmap font). |
| `src/engine_host.rs` | 384 | Runs the engine on its OWN thread with a big stack and talks to it through messages. |
| `src/ffi.rs` | 215 | C ABI for the shell (Go/cgo, C, C++). All functions are panic-safe: a panic never crosses FFI. |
| `src/forms.rs` | 239 | HTML form submission: collect successful controls (HTML spec 4.10.21), urlencode, build GET/POST request. |
| `src/input.rs` | 393 | Text editing model for <input>/<textarea>: caret, selection, typing, delete, word/line movement, |
| `src/js_engine.rs` | 44 | JS engine abstraction: DOM/bridge code should depend on `JsEngine`, never on boa directly. |
| `src/lib.rs` | 34 | Sparrow Engine: Rust browser engine embedded in the Sparrow shell. |
| `src/safety.rs` | 137 | Crash/hang protection helpers (std only). |
| `src/sparrow_engine.rs` | 257 | SparrowEngine: one page = DOM + styles + layout + JS + media. Everything is RAM-only. |
| `src/sparrow_engine/bridge.rs` | 265 | Child module of `sparrow_engine` (so it can touch private fields). |
| `src/sparrow_engine/media_bridge.rs` | 225 | Child module of `sparrow_engine`: connects <video>/<audio> elements to `dro_media::Media`. |
| `tests/fixes.rs` | 43 | Regression tests for the fixes in CHANGES.md. Run: cargo test --test fixes |
| `tests/regression.rs` | 53 |  |
| `tests/smoke.rs` | 55 | Smoke tests: run `cargo test --test smoke` after the crate builds. |
| `tools/integration_check.py` | 139 |  |

---

## Pipeline walkthrough

### Network

Files: `dro_net.rs`

Blocking reqwest client, `Request`/`send`, `fetch`, `fetch_max`, `fetch_range`, `post_form`, `par_map`. Cookie jar (`RamJar`), cache, HSTS, https-first, private-network sub-resource guard.

### Charset

Files: `dro_charset.rs`

BOM, `Content-Type` charset, `<meta charset>` sniffing, `encoding_rs` decode.

### HTML

Files: `dro_html.rs`

html5ever tree builder into the arena `Dom`.

### DOM

Files: `dro_dom.rs`

`Node { kind, parent, children }`, `NodeKind::{Document, Element, Text, Comment}`.

### CSS

Files: `dro_css.rs, dro_css_ext.rs`

Parser, selectors, cascade, `Style`; extensions for calc/vars/media queries.

### Layout

Files: `dro_layout.rs`

`compute(dom, styles, width, base, image_sizes)` builds a taffy tree and a flat list of paint items.

### Fonts

Files: `dro_font.rs`

fontdb discovery, rustybuzz shaping, bidi runs, line breaks, web fonts, caret stops.

### Paint

Files: `dro_paint.rs`

`paint(layout, w, h, scroll, images)`; `decode_image`.

### JS

Files: `dro_js.rs, js_engine.rs, dom_bootstrap.js, dro_dom_js.rs`

Boa context with loop and recursion limits; DOM shim in JS; native bridge in Rust.

### Media

Files: `dro_media.rs, dro_media_ui.rs, media_bridge.rs`

symphonia decode, cpal output, GIF frames, control bar overlay.

### Safety

Files: `safety.rs`

RAM log ring, panic hook, `run_guarded`, `run_big_stack`, `run_with_timeout`, `Deadline`, `limits`.

---

## Module reference

Generated from `///` docs and `pub` items. Test modules are excluded.

### `src/browser/bookmark.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Bookmarks` | `pub struct Bookmarks` |  |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` |  |
| fn | `add` | `pub fn add(&self, url: &str, title: &str) -> Result<(), String>` |  |
| fn | `remove` | `pub fn remove(&self, url: &str) -> Result<(), String>` |  |
| fn | `contains` | `pub fn contains(&self, url: &str) -> Result<bool, String>` |  |
| fn | `list` | `pub fn list(&self) -> Result<Vec<(String, String)>, String>` |  |

### `src/browser/history.rs`

History is RAM-only by design: nothing is written to disk, everything is gone on exit.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Entry` | `pub struct Entry` |  |
| struct | `History` | `pub struct History` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `open` | `pub fn open(_path: &str) -> Result<Self, String>` | Compatibility with the old SQLite API: the path is ignored on purpose. |
| fn | `add` | `pub fn add(&mut self, url: &str, title: &str) -> Result<(), String>` |  |
| fn | `search` | `pub fn search(&self, q: &str, limit: usize) -> Result<Vec<(String, String)>, String>` |  |
| fn | `recent` | `pub fn recent(&self, limit: usize) -> Vec<Entry>` |  |
| fn | `delete_since` | `pub fn delete_since(&mut self, secs_ago: i64)` |  |
| fn | `clear` | `pub fn clear(&mut self) -> Result<(), String>` |  |
| fn | `len` | `pub fn len(&self) -> usize` |  |
| fn | `is_empty` | `pub fn is_empty(&self) -> bool` |  |

Tests: `add_search_clear`, `ignores_blank_and_caps`

### `src/browser/mod.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| mod | `bookmark` | `pub mod bookmark` |  |
| mod | `history` | `pub mod history` |  |
| mod | `omnibox` | `pub mod omnibox` |  |
| mod | `session` | `pub mod session` |  |
| mod | `settings` | `pub mod settings` |  |
| mod | `tabs` | `pub mod tabs` |  |

### `src/browser/omnibox.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| const | `SEARCH` | `pub const SEARCH: &str = "https://duckduckgo.com/?q="` | Search engine (not Google/MS). Change if you want. |
| fn | `resolve` | `pub fn resolve(input: &str) -> String` |  |

### `src/browser/session.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Session` | `pub struct Session` | Save/restore open tabs between runs. |
| struct | `Restored` | `pub struct Restored` |  |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` |  |
| fn | `save` | `pub fn save(&mut self, tabs: &mut Tabs) -> Result<(), String>` | Snapshot all tabs (blank tabs are skipped). |
| fn | `load` | `pub fn load(&self) -> Result<Restored, String>` | Read the saved list. The shell decides how to load (recommended: load only the active tab, others lazily). |
| fn | `clear` | `pub fn clear(&self) -> Result<(), String>` |  |

### `src/browser/settings.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Settings` | `pub struct Settings` | Simple persistent key/value settings (SQLite). Keys used by the shell: "search_engine", "homepage", "js_enabled", "theme". |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` |  |
| fn | `get` | `pub fn get(&self, key: &str) -> Result<Option<String>, String>` |  |
| fn | `get_or` | `pub fn get_or(&self, key: &str, default: &str) -> String` |  |
| fn | `set` | `pub fn set(&self, key: &str, value: &str) -> Result<(), String>` |  |
| fn | `remove` | `pub fn remove(&self, key: &str) -> Result<(), String>` |  |

### `src/browser/store.rs`

Tiny persistent store for the ONLY data that lives on disk: bookmarks and settings.
Plain text file (`key<TAB>value` per line, `\\ \t \n` escaped), atomic write (tmp + rename). No SQLite.
History, sessions, cache are RAM-only by design.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Store` | `pub struct Store` |  |
| fn | `memory` | `pub fn memory() -> Self` |  |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` | Missing file = empty store. Corrupt lines are skipped, never fatal. |
| fn | `get` | `pub fn get(&self, k: &str) -> Option<&str>` |  |
| fn | `set` | `pub fn set(&mut self, k: &str, v: &str) -> Result<(), String>` |  |
| fn | `remove` | `pub fn remove(&mut self, k: &str) -> Result<bool, String>` |  |
| fn | `with_prefix` | `pub fn with_prefix(&self, prefix: &str) -> Vec<(String, String)>` |  |
| fn | `len` | `pub fn len(&self) -> usize` |  |
| struct | `Bookmarks` | `pub struct Bookmarks` | Same API as the old SQLite `Bookmarks`. |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` |  |
| fn | `memory` | `pub fn memory() -> Self` |  |
| fn | `add` | `pub fn add(&mut self, url: &str, title: &str) -> Result<(), String>` |  |
| fn | `remove` | `pub fn remove(&mut self, url: &str) -> Result<(), String>` |  |
| fn | `contains` | `pub fn contains(&self, url: &str) -> Result<bool, String>` |  |
| fn | `list` | `pub fn list(&self) -> Result<Vec<(String, String)>, String>` | (url, title), sorted by url. |
| struct | `Settings` | `pub struct Settings` | Same API as the old SQLite `Settings`. |
| fn | `open` | `pub fn open(path: &str) -> Result<Self, String>` |  |
| fn | `memory` | `pub fn memory() -> Self` |  |
| fn | `get` | `pub fn get(&self, k: &str) -> Result<Option<String>, String>` |  |
| fn | `get_or` | `pub fn get_or(&self, k: &str, default: &str) -> String` |  |
| fn | `set` | `pub fn set(&mut self, k: &str, v: &str) -> Result<(), String>` |  |
| fn | `remove` | `pub fn remove(&mut self, k: &str) -> Result<(), String>` |  |

Tests: `corrupt_lines_skipped_and_prefixes_isolated`, `memory_mode_never_touches_disk`, `roundtrip_with_special_chars`, `settings_defaults_and_remove`

### `src/browser/tabs.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Tab` | `pub struct Tab` |  |
| struct | `Tabs` | `pub struct Tabs` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `open` | `pub fn open(&mut self) -> u32` |  |
| fn | `close` | `pub fn close(&mut self, id: u32)` |  |
| fn | `switch` | `pub fn switch(&mut self, id: u32)` |  |
| fn | `list` | `pub fn list(&self) -> &[Tab]` |  |
| fn | `active` | `pub fn active(&mut self) -> &mut Tab` |  |
| fn | `navigate` | `pub fn navigate(&mut self, input: &str) -> Result<(), String>` | Address bar text -> navigate active tab. |
| fn | `go_back` | `pub fn go_back(&mut self) -> Result<(), String>` |  |
| fn | `go_forward` | `pub fn go_forward(&mut self) -> Result<(), String>` |  |

### `src/dro_blocklist.rs`

Tracker / ad blocking for sub-resources (scripts, images, CSS, media). Top-level navigation is never blocked.
Rules: hosts-file lines (`0.0.0.0 ads.example.com`), `||domain^`, plain domains, and `/path/` substring patterns.
Pure std + `url`. Shell can load bigger lists (EasyList-hosts style) with `with_global(|b| b.load_list(text))`.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Blocker` | `pub struct Blocker` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `empty` | `pub fn empty() -> Self` |  |
| fn | `add_rule` | `pub fn add_rule(&mut self, rule: &str)` |  |
| fn | `load_list` | `pub fn load_list(&mut self, text: &str)` | Whitespace/newline separated rules, `#`/`!` comments allowed. |
| fn | `rule_count` | `pub fn rule_count(&self) -> usize` |  |
| fn | `is_blocked` | `pub fn is_blocked(&self, url: &str, page_url: &str) -> bool` | Is `url` (a sub-resource of the page at `page_url`) blocked? First-party requests are never blocked. |
| fn | `with_global` | `pub fn with_global<R>(f: impl FnOnce(&mut Blocker) -> R) -> Option<R>` |  |
| fn | `global_blocked` | `pub fn global_blocked(url: &str, page_url: &str) -> bool` | Sub-resource check used by the network layer. |

Tests: `blocks_third_party_trackers`, `first_party_never_blocked`, `list_formats`

### `src/dro_cache.rs`

RAM-only HTTP cache (never written to disk): Cache-Control max-age / no-store / no-cache / immutable,
ETag + Last-Modified revalidation (304), byte-bounded LRU. GET 200 responses only.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Entry` | `pub struct Entry` |  |
| struct | `Headers` | `pub struct Headers<'a>` | Response headers relevant to caching (pass `None` for absent ones). |
| struct | `Policy` | `pub struct Policy` |  |
| fn | `parse_cache_control` | `pub fn parse_cache_control(h: &str) -> Policy` |  |
| enum | `Lookup` | `pub enum Lookup<'a>` |  |
| struct | `Cache` | `pub struct Cache` |  |
| fn | `new` | `pub fn new(max_bytes: usize) -> Self` |  |
| fn | `lookup` | `pub fn lookup(&mut self, url: &str) -> Lookup<'_>` |  |
| fn | `lookup_at` | `pub fn lookup_at(&mut self, url: &str, now: Instant) -> Lookup<'_>` |  |
| fn | `store` | `pub fn store(&mut self, url: &str, status: u16, content_type: &str, body: Vec<u8>, h: Headers<'_>) -> bool` | Store a 200 GET response. Returns false when policy/size says not to cache it. |
| fn | `store_at` | `pub fn store_at(&mut self, url: &str, status: u16, content_type: &str, body: Vec<u8>, h: Headers<'_>, now: Instant) -> bool` |  |
| fn | `refresh` | `pub fn refresh(&mut self, url: &str, h: Headers<'_>) -> Option<&Entry>` | Server answered 304: the stored body is valid again (optionally with new max-age). |
| fn | `refresh_at` | `pub fn refresh_at(&mut self, url: &str, h: Headers<'_>, now: Instant) -> Option<&Entry>` |  |
| fn | `remove` | `pub fn remove(&mut self, url: &str)` |  |
| fn | `clear` | `pub fn clear(&mut self)` |  |
| fn | `len` | `pub fn len(&self) -> usize` |  |
| fn | `bytes` | `pub fn bytes(&self) -> usize` |  |

Tests: `fresh_then_stale_then_refresh`, `lru_eviction_by_bytes_and_entry_cap`, `parses_policy`, `policy_rules`, `touch_protects_recent_and_replace_updates_size`

### `src/dro_charset.rs`

Charset detection + decoding without extra crates.
Supports: UTF-8 (+BOM), UTF-16 LE/BE (BOM), ISO-8859-1, windows-1252, ASCII.
Unknown labels (gbk, shift_jis, ...) fall back to lossy UTF-8.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `charset_from_header` | `pub fn charset_from_header(ct: &str) -> Option<String>` | Extract `charset=xxx` from a Content-Type header value. |
| fn | `charset_from_meta` | `pub fn charset_from_meta(body: &[u8]) -> Option<String>` | Look for `<meta charset=..>` / `<meta http-equiv content="..charset=..">` in the first 2 KB. |
| fn | `decode` | `pub fn decode(body: &[u8], content_type: &str) -> String` |  |

Tests: `decode_bom_and_utf8`, `decode_latin1_and_cp1252`, `header_charset`, `meta_charset`

### `src/dro_css.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Color` | `pub struct Color(pub u8, pub u8, pub u8)` |  |
| enum | `Display` | `pub enum Display` |  |
| struct | `Style` | `pub struct Style` |  |
| struct | `Rule` | `pub struct Rule` |  |
| const | `UA` | `pub const UA: &str = "html,body,div,p,h1,h2,h3,h4,h5,h6,ul,ol,li,section,article,header,footer,nav,main,form,table,tr,pre,blockquote` |  |
| fn | `parse_color` | `pub fn parse_color(s: &str) -> Option<Color>` |  |
| fn | `parse` | `pub fn parse(css: &str) -> Vec<Rule>` |  |
| fn | `compute` | `pub fn compute(dom: &Dom, rules: &[Rule]) -> Vec<Option<Style>>` |  |

### `src/dro_css_ext.rs`

CSS pre-processor: turns modern CSS into the subset `dro_css::parse` understands.
Handles: @media (min/max-width/height, orientation, screen/print), @supports (assumed true),
`:root` -> `html`, custom properties `var(--x, fallback)`, `vw/vh/vmin/vmax` units, `calc()`.
Pure text -> text, no dependencies. Call as `dro_css::parse(&preprocess(css, vw, vh))`.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `preprocess` | `pub fn preprocess(css: &str, vw: f32, vh: f32) -> String` |  |
| fn | `media_matches` | `pub fn media_matches(q: &str, vw: f32, vh: f32) -> bool` |  |

Tests: `at_rules_dropped`, `calc_eval`, `media_filtering`, `media_queries`, `units`, `vars`

### `src/dro_dom.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| type | `NodeId` | `pub type NodeId = usize` |  |
| enum | `NodeKind` | `pub enum NodeKind` |  |
| struct | `Node` | `pub struct Node` |  |
| struct | `Dom` | `pub struct Dom` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `create` | `pub fn create(&mut self, kind: NodeKind) -> NodeId` |  |
| fn | `detach` | `pub fn detach(&mut self, id: NodeId)` |  |
| fn | `append_child` | `pub fn append_child(&mut self, parent: NodeId, child: NodeId)` |  |
| fn | `insert_before` | `pub fn insert_before(&mut self, sibling: NodeId, child: NodeId)` |  |
| fn | `tag` | `pub fn tag(&self, id: NodeId) -> Option<&str>` |  |
| fn | `attr` | `pub fn attr(&self, id: NodeId, name: &str) -> Option<&str>` |  |
| fn | `descendants` | `pub fn descendants(&self, id: NodeId) -> Vec<NodeId>` | Preorder list of all descendants |
| fn | `text_content` | `pub fn text_content(&self, id: NodeId) -> String` |  |
| fn | `find_tag` | `pub fn find_tag(&self, tag: &str) -> Option<NodeId>` |  |

### `src/dro_dom_js.rs`

Bridge between our Rust DOM and the JS DOM layer (`dom_bootstrap.js`).
Only uses `JsRuntime::eval` (no boa-specific API) so it is robust to boa version changes.
Flow: Rust DOM -> JS literal -> hydrate -> scripts mutate JS DOM -> `__serialize()` -> html5ever re-parse.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `js_string` | `pub fn js_string(s: &str) -> String` | Escape a Rust string as a double-quoted JS string literal. |
| fn | `dom_literal` | `pub fn dom_literal(dom: &Dom) -> Result<String, String>` | DOM -> `[[kind, text_or_tag, [[attr,val]..], [childIds..]], ...]` (index == NodeId). kind: 0 document, 1 element, 2 text, 3 comment. Script  |
| fn | `install` | `pub fn install(rt: &mut JsRuntime, dom: &Dom, url: &str, _title: &str, vw: u32, vh: u32) -> Result<(), String>` |  |
| fn | `lifecycle` | `pub fn lifecycle(rt: &mut JsRuntime)` | Fire DOMContentLoaded, run pending timers (max 1s virtual time), fire load. |
| fn | `run_timers` | `pub fn run_timers(rt: &mut JsRuntime, ms: u32) -> usize` | Advance the virtual clock by `ms` and run due timers. Returns how many ran. |
| fn | `serialize_if_dirty` | `pub fn serialize_if_dirty(rt: &mut JsRuntime) -> Result<Option<String>, String>` | `Some(html)` only if JS changed the DOM since the last call. |
| fn | `take_navigation` | `pub fn take_navigation(rt: &mut JsRuntime) -> Option<String>` | Navigation requested by JS (`location.href = ..`, clicked link, `window.open`). |
| fn | `fire` | `pub fn fire(rt: &mut JsRuntime, element_id: &str, event: &str) -> Result<bool, String>` | Dispatch a DOM event on the element with the given `id` attribute. |

Tests: `escapes`, `literal_shape`

### `src/dro_font.rs`

dro_font v2 — real text for Sparrow: system fonts (fontdb), OpenType shaping (rustybuzz, Bangla conjuncts),
bidi (unicode-bidi), CSS font-family lists, weights 100-900, italic (real or synthetic), web fonts (TTF/OTF),
glyph-mask cache with 1/4-pixel horizontal positioning, caret/cluster positions (for text input),
letter-spacing, underline/strike, line-break opportunities, RAM-light lazy face loading.
Cargo: unicode-bidi = "0.3", unicode-linebreak = "0.1" (plus existing fontdb, rustybuzz, tiny-skia).
Old API kept: Fonts::{new,count,has_fonts,shape,measure,metrics,draw}, with/available/measure/draw_text.
NOT yet: WOFF/WOFF2 (decompress first), colour emoji, vertical text, hinting, LCD subpixel AA.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `TextStyle` | `pub struct TextStyle` |  |
| fn | `new` | `pub fn new(size: f32) -> Self` |  |
| fn | `bold` | `pub fn bold(mut self, b: bool) -> Self` |  |
| fn | `weight` | `pub fn weight(mut self, w: u16) -> Self` |  |
| fn | `italic` | `pub fn italic(mut self, i: bool) -> Self` |  |
| fn | `families` | `pub fn families(mut self, css: &str) -> Self` |  |
| fn | `spacing` | `pub fn spacing(mut self, px: f32) -> Self` |  |
| fn | `parse_families` | `pub fn parse_families(css: &str) -> Vec<String>` | Parse a CSS `font-family` value: `Roboto, "Helvetica Neue", sans-serif`. |
| struct | `G` | `pub struct G` |  |
| struct | `Shaped` | `pub struct Shaped` |  |
| struct | `Metrics` | `pub struct Metrics` |  |
| fn | `bidi_runs` | `pub fn bidi_runs(text: &str) -> Vec<(Range<usize>, bool)>` | Visual-order runs `(byte range, is_rtl)`. Pure LTR text takes the fast path. |
| fn | `line_breaks` | `pub fn line_breaks(text: &str) -> Vec<(usize, bool)>` | Byte offsets where a line may break (`true` = forced break). UAX #14. |
| fn | `index_at_x` | `pub fn index_at_x(stops: &[(usize, f32)], x: f32) -> usize` | Nearest caret stop (byte index) to pixel `x`. |
| struct | `Fonts` | `pub struct Fonts` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `count` | `pub fn count(&self) -> usize` | Number of fonts found on this system (plus registered web fonts). |
| fn | `loaded_faces` | `pub fn loaded_faces(&self) -> usize` | Faces actually loaded into RAM (lazy: only fonts that were needed). |
| fn | `has_fonts` | `pub fn has_fonts(&mut self) -> bool` |  |
| fn | `shape` | `pub fn shape(&mut self, text: &str, size: f32, bold: bool) -> Shaped` |  |
| fn | `shape_style` | `pub fn shape_style(&mut self, text: &str, st: &TextStyle) -> Shaped` |  |
| fn | `measure` | `pub fn measure(&mut self, text: &str, size: f32, bold: bool) -> f32` |  |
| fn | `measure_style` | `pub fn measure_style(&mut self, text: &str, st: &TextStyle) -> f32` |  |
| fn | `caret_stops` | `pub fn caret_stops(&mut self, text: &str, st: &TextStyle) -> Vec<(usize, f32)>` | Caret stops `(byte index, x)` in logical order, always starting at 0 and ending at `text.len()`. Ligatures / Bangla conjuncts are one stop ( |
| fn | `metrics_style` | `pub fn metrics_style(&mut self, st: &TextStyle) -> Metrics` |  |
| fn | `metrics` | `pub fn metrics(&mut self, size: f32, bold: bool) -> (f32, f32)` | (ascent, descent) in px for the primary face. |
| fn | `register_web_font` | `pub fn register_web_font(&mut self, alias: &str, data: Vec<u8>, weight: u16, italic: bool) -> bool` | Register a TTF/OTF/TTC web font under a CSS family `alias`. WOFF/WOFF2 must be decompressed first. |
| fn | `clear_web_fonts` | `pub fn clear_web_fonts(&mut self)` | Drop all web fonts (call on navigation): frees their RAM. |
| fn | `draw` | `pub fn draw(&mut self, pm: &mut Pixmap, s: &Shaped, x: f32, baseline: f32, rgb: (u8, u8, u8), bold: bool)` | Draw shaped text with its baseline at `baseline`, left edge at `x`. |
| fn | `draw_decoration` | `pub fn draw_decoration(&mut self, pm: &mut Pixmap, x: f32, baseline: f32, width: f32, st: &TextStyle, rgb: (u8, u8, u8), underline: bool, strike: b...` |  |
| fn | `with` | `pub fn with<R>(f: impl FnOnce(&mut Fonts) -> R) -> R` |  |
| fn | `available` | `pub fn available() -> bool` | True when at least one usable system font exists (else callers use the bitmap font). |
| fn | `measure` | `pub fn measure(text: &str, size: f32, bold: bool) -> f32` |  |
| fn | `measure_style` | `pub fn measure_style(text: &str, st: &TextStyle) -> f32` |  |
| fn | `caret_stops` | `pub fn caret_stops(text: &str, st: &TextStyle) -> Vec<(usize, f32)>` |  |
| fn | `register_web_font` | `pub fn register_web_font(alias: &str, data: Vec<u8>, weight: u16, italic: bool) -> bool` |  |
| fn | `clear_web_fonts` | `pub fn clear_web_fonts()` |  |
| fn | `draw_text` | `pub fn draw_text(pm: &mut Pixmap, text: &str, x: f32, y: f32, box_h: f32, size: f32, bold: bool, rgb: (u8, u8, u8)) -> bool` | Draw `text` vertically centred in a box of height `box_h` whose top is `y`. Returns false if no fonts. |
| fn | `draw_text_style` | `pub fn draw_text_style(pm: &mut Pixmap, text: &str, x: f32, y: f32, box_h: f32, st: &TextStyle, rgb: (u8, u8, u8)) -> bool` | Same with a full style (family list, weight, italic, letter-spacing). |

Tests: `bidi_fast_path_and_rtl`, `caret_stops_are_monotonic_and_cluster_aware`, `css_family_parsing`, `index_at_x_picks_nearest`, `latin_and_bangla_shape`, `lazy_loading_latin_loads_few_faces`, `line_break_opportunities`, `mask_raster_and_blit`, `renders_pixels`, `rtl_shaping_does_not_panic`, `style_builder_clamps`, `styles_change_width_and_cache_hits`, `synthetic_italic_shifts_top`, `web_font_rejects_garbage_and_woff`

### `src/dro_html.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `H` | `pub struct H` |  |
| fn | `parse` | `pub fn parse(html: &str) -> Result<Dom, String>` |  |

### `src/dro_js.rs`

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `JsRuntime` | `pub struct JsRuntime` |  |
| fn | `new` | `pub fn new() -> Result<Self, String>` |  |
| fn | `eval` | `pub fn eval(&mut self, code: &str) -> Result<String, String>` |  |
| fn | `take_logs` | `pub fn take_logs(&self) -> Vec<String>` |  |

### `src/dro_layout.rs`

Sparrow layout: DOM + computed styles -> positioned paint items (via taffy).

Public API (unchanged): `Paint`, `Item`, `Layout`, `compute`.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| enum | `Paint` | `pub enum Paint` |  |
| struct | `Item` | `pub struct Item` |  |
| struct | `Layout` | `pub struct Layout` |  |
| fn | `is_virama` | `pub(crate) fn is_virama(c: char) -> bool` |  |
| fn | `cells` | `pub(crate) fn cells(c: char, after_virama: bool) -> f32` | Approximate width of one char in "cells" (1 cell = one `advance(size)`). Combining marks take no space, a consonant joined by a virama (Beng |
| fn | `compute` | `pub fn compute(dom: &Dom, st: &[Option<Style>], width: f32, base: &str, imgs: &HashMap<String, (u32, u32)>) -> Result<Layout, String>` |  |

### `src/dro_media.rs`

Pure-Rust media player (no FFmpeg, no C).
Audio : symphonia (mp3/aac/flac/vorbis/ogg/wav/mp4-audio/mkv-audio) -> linear resampler -> cpal output.
Clock : audio clock is master (like Chromium). No-audio media uses a wall clock.
Video : animated GIF decoded in pure Rust; MP4 video is probed (size/fps/codec) but needs a decoder backend
(AV1 via rav1d is the planned pure-Rust one). Audio of MP4 files plays today.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Frame` | `pub struct Frame` |  |
| struct | `VideoInfo` | `pub struct VideoInfo` |  |
| struct | `Media` | `pub struct Media` |  |
| fn | `probe_mp4` | `pub fn probe_mp4(bytes: &[u8]) -> Option<VideoInfo>` | MP4 video track info (size, fps, codec) with the pure-Rust `mp4` crate. |
| fn | `open` | `pub fn open(bytes: Vec<u8>, hint: &str) -> Result<Media, String>` | `hint` = file extension or mime subtype ("mp3", "mp4", "ogg", ...), may be "". |
| fn | `play` | `pub fn play(&mut self)` |  |
| fn | `pause` | `pub fn pause(&mut self)` |  |
| fn | `paused` | `pub fn paused(&self) -> bool` |  |
| fn | `ended` | `pub fn ended(&self) -> bool` |  |
| fn | `set_volume` | `pub fn set_volume(&mut self, v: f32)` |  |
| fn | `volume` | `pub fn volume(&self) -> f32` |  |
| fn | `set_muted` | `pub fn set_muted(&mut self, m: bool)` |  |
| fn | `muted` | `pub fn muted(&self) -> bool` |  |
| fn | `has_audio` | `pub fn has_audio(&self) -> bool` |  |
| fn | `seek` | `pub fn seek(&mut self, secs: f64)` |  |
| fn | `current_time` | `pub fn current_time(&self) -> f64` | Current playback position in seconds (audio clock when audio exists). |
| fn | `frame` | `pub fn frame(&self) -> Option<&Frame>` | Frame to show right now (animated GIF). None for audio-only / undecodable video. |
| fn | `video_supported` | `pub fn video_supported(&self) -> bool` | True when the video track can be shown (a decoder backend exists). |

Tests: `bad_input_is_error`, `channel_conversion`, `resampler_ratio`, `wall_clock`

### `src/dro_media_ui.rs`

Chromium-style media control bar: draw + hit-test. Coordinates are relative to the media box.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| const | `BAR_H` | `pub const BAR_H: f32 = 36.0` |  |
| enum | `Action` | `pub enum Action` |  |
| struct | `State` | `pub struct State` |  |
| fn | `fmt_time` | `pub fn fmt_time(s: f64) -> String` |  |
| fn | `hit` | `pub fn hit(x: f32, y: f32, w: f32, h: f32, duration: f64) -> Option<Action>` | `x`,`y` relative to the media box (w x h). Returns the action for a click on the bar. |
| fn | `draw` | `pub fn draw(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, s: &State)` |  |

Tests: `draws_without_panic`, `hits`, `time_format`

### `src/dro_net.rs`

dro_net.rs — Sparrow network stack. RAM-only: cookies, cache, HSTS never touch disk.
Cargo: reqwest = { version = "0.11", default-features = false,
features = ["blocking","rustls-tls","cookies","gzip","brotli","deflate","http2"] }  (>= 0.11.18)
Old API kept: Response, fetch, fetch_sub, par_map(urls, f), decode, resolve.
New: fetch_max, fetch_range, post_form, send(Request), par_map_page, RAM cookie jar (export/import
for a login vault), HTTP cache + revalidation, HSTS, HTTPS-first, retries, Chrome-like headers,
referrer policy, private-network protection, data: URLs, MIME sniffing, stats.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Response` | `pub struct Response` |  |
| struct | `Config` | `pub struct Config` |  |
| fn | `config` | `pub fn config() -> Config` |  |
| fn | `set_config` | `pub fn set_config(c: Config)` |  |
| struct | `Stats` | `pub struct Stats` |  |
| fn | `stats` | `pub fn stats() -> Stats` |  |
| fn | `set_page` | `pub fn set_page(u: &str)` |  |
| fn | `reg_domain` | `pub fn reg_domain(host: &str) -> String` |  |
| fn | `same_site` | `pub fn same_site(a: &str, b: &str) -> bool` |  |
| struct | `Cookie` | `pub struct Cookie` |  |
| struct | `RamJar` | `pub struct RamJar` |  |
| fn | `parse_http_date` | `pub fn parse_http_date(s: &str) -> Option<u64>` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `len` | `pub fn len(&self) -> usize` |  |
| fn | `clear` | `pub fn clear(&self)` |  |
| fn | `clear_host` | `pub fn clear_host(&self, host: &str)` |  |
| fn | `export` | `pub fn export(&self, only_sites: Option<&[&str]>, session_ttl: u64) -> String` | Tab-separated lines. `only_sites`: registrable domains to export (None = all). Session cookies get `now + session_ttl` so a login survives a |
| fn | `import` | `pub fn import(&self, text: &str) -> usize` |  |
| fn | `cookies_export` | `pub fn cookies_export(only_sites: Option<&[&str]>, session_ttl: u64) -> String` |  |
| fn | `cookies_import` | `pub fn cookies_import(text: &str) -> usize` |  |
| fn | `cookies_clear` | `pub fn cookies_clear()` |  |
| fn | `cookies_clear_host` | `pub fn cookies_clear_host(host: &str)` |  |
| fn | `cookie_count` | `pub fn cookie_count() -> usize` |  |
| fn | `cache_clear` | `pub fn cache_clear()` |  |
| fn | `cache_bytes` | `pub fn cache_bytes() -> usize` |  |
| struct | `Request` | `pub struct Request` |  |
| fn | `get` | `pub fn get(url: &str) -> Self` |  |
| fn | `sub` | `pub fn sub(url: &str, page: &str) -> Self` |  |
| fn | `post_form` | `pub fn post_form(url: &str, fields: &[(String, String)], page: Option<&str>) -> Self` |  |
| fn | `post` | `pub fn post(url: &str, content_type: &str, body: Vec<u8>, page: Option<&str>) -> Self` |  |
| fn | `send` | `pub fn send(r: &Request) -> Result<Response, String>` | Full pipeline: data: URLs, blocklist, private-network guard, HSTS / HTTPS-first, cache, retries. |
| fn | `fetch` | `pub fn fetch(url: &str) -> Result<Response, String>` | Main page: any status is returned (error pages are rendered). |
| fn | `fetch_max` | `pub fn fetch_max(url: &str, max: u64) -> Result<Response, String>` |  |
| fn | `fetch_range` | `pub fn fetch_range(url: &str, start: u64, end: Option<u64>, max: u64) -> Result<Response, String>` | Byte range (media). `end` inclusive. Server may answer 206 or 200. |
| fn | `fetch_sub` | `pub fn fetch_sub(url: &str) -> Result<Response, String>` | Subresource (CSS/img/js): short timeout, 2xx only, cached, tracker-blocked. |
| fn | `fetch_sub_page` | `pub fn fetch_sub_page(url: &str, page: &str) -> Result<Response, String>` |  |
| fn | `post_form` | `pub fn post_form(url: &str, fields: &[(String, String)], page: Option<&str>) -> Result<Response, String>` |  |
| fn | `par_map` | `pub fn par_map<T: Send>(urls: &[String], f: impl Fn(Response) -> Option<T> + Sync) -> Vec<Option<T>>` | Parallel subresource fetch (order preserved). Uses the current page for blocking/referrer. |
| fn | `par_map_page` | `pub fn par_map_page<T: Send>(urls: &[String], page: &str, f: impl Fn(Response) -> Option<T> + Sync) -> Vec<Option<T>>` |  |
| fn | `clear_all` | `pub fn clear_all()` | Forget everything held in RAM (call on exit or "clear browsing data"). |
| fn | `sniff_mime` | `pub fn sniff_mime(body: &[u8], declared: &str) -> String` | Fix wrong/missing Content-Type by magic bytes. |
| fn | `decode` | `pub fn decode(body: &[u8], content_type: &str) -> String` | Decode using BOM, Content-Type charset, then <meta charset> sniff, else UTF-8. |
| fn | `resolve` | `pub fn resolve(base: &str, href: &str) -> Option<String>` |  |

Tests: `cache_fresh_and_stale`, `cookie_basic_and_security`, `cookie_export_import_roundtrip`, `cookie_rejections_and_delete`, `data_urls_and_base64`, `domains`, `hsts_rules`, `http_dates`, `par_map_keeps_order`, `private_hosts`, `referrer_policy`, `sniff_and_decode`, `third_party_cookie_blocking`

### `src/dro_paint.rs`

Sparrow paint: layout items -> RGBA pixels (tiny-skia + built-in 5x7 bitmap font).

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `advance` | `pub fn advance(size: f32) -> f32` | Horizontal advance of one character cell. `dro_layout` uses this for measuring, so it must stay in sync with how `text()` places glyphs. |
| fn | `decode_image` | `pub fn decode_image(bytes: &[u8]) -> Result<Pixmap, String>` | Decode PNG/JPEG/GIF into a premultiplied pixmap. Oversized images are rejected before decoding. |
| fn | `paint` | `pub fn paint(l: &Layout, w: u32, h: u32, scroll: f32, images: &HashMap<String, Pixmap>) -> Result<Vec<u8>, String>` |  |

### `src/engine_host.rs`

Runs the engine on its OWN thread with a big stack and talks to it through messages.
- `EngineHost::call(closure)` : run any code on the engine thread (panics are caught -> "Page crashed, reload?").
- `Cmd` / `Reply` + `execute()` : the full engine API as plain data, with a binary codec (`to_bytes/from_bytes`),
so the same protocol can later run over a pipe to a child process (real crash isolation).
Limits: a big stack makes stack overflow far less likely but cannot make it impossible; only a separate
process survives a hard overflow / OOM. Panics are fully handled.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| const | `CRASH_MSG` | `pub const CRASH_MSG: &str = "Page crashed, reload?"` |  |
| const | `DEFAULT_STACK` | `pub const DEFAULT_STACK: usize = 256 * 1024 * 1024; // address space reserve; pages are committed lazily` |  |
| enum | `Cmd` | `pub enum Cmd` |  |
| enum | `Reply` | `pub enum Reply` |  |
| fn | `execute` | `pub fn execute(e: &mut SparrowEngine, c: Cmd) -> Reply` | Execute one command on an engine. Never panics outward (engine methods are already guarded). |
| fn | `to_bytes` | `pub fn to_bytes(&self) -> Vec<u8>` |  |
| fn | `from_bytes` | `pub fn from_bytes(b: &[u8]) -> Result<Cmd, String>` |  |
| fn | `to_bytes` | `pub fn to_bytes(&self) -> Vec<u8>` |  |
| fn | `from_bytes` | `pub fn from_bytes(b: &[u8]) -> Result<Reply, String>` |  |
| struct | `EngineHost` | `pub struct EngineHost` |  |
| fn | `spawn` | `pub fn spawn() -> Result<Self, String>` |  |
| fn | `spawn_with_stack` | `pub fn spawn_with_stack(stack: usize) -> Result<Self, String>` | Tries `stack` bytes, then smaller stacks if the OS refuses (32-bit, low memory). |
| fn | `call` | `pub fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut SparrowEngine) -> R + Send + 'static) -> Result<R, String>` |  |
| fn | `call_timeout` | `pub fn call_timeout<R: Send + 'static>(&self, f: impl FnOnce(&mut SparrowEngine) -> R + Send + 'static, timeout: Option<Duration>) -> Result<R, Str...` | With a timeout the call returns "engine busy" but the engine keeps working on the job. |
| fn | `exec` | `pub fn exec(&self, c: Cmd) -> Reply` | Run one protocol command. |
| fn | `is_alive` | `pub fn is_alive(&self) -> bool` |  |
| struct | `RemoteEngine` | `pub struct RemoteEngine` | Same method names/signatures as `SparrowEngine`, but every call runs on the engine thread. `ffi.rs` only needs `SparrowEngine` -> `RemoteEng |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `is_crashed` | `pub fn is_crashed(&self) -> bool` |  |
| fn | `url` | `pub fn url(&self) -> &str` |  |
| fn | `title` | `pub fn title(&self) -> &str` |  |
| fn | `viewport` | `pub fn viewport(&self) -> (u32, u32)` |  |
| fn | `recover` | `pub fn recover(&mut self)` |  |
| fn | `load_url` | `pub fn load_url(&mut self, u: &str) -> Result<(), String>` |  |
| fn | `load_html` | `pub fn load_html(&mut self, html: &str, base: &str) -> Result<(), String>` |  |
| fn | `reload` | `pub fn reload(&mut self) -> Result<(), String>` |  |
| fn | `resize` | `pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String>` |  |
| fn | `render` | `pub fn render(&mut self) -> Result<Vec<u8>, String>` |  |
| fn | `eval_js` | `pub fn eval_js(&mut self, code: &str) -> Result<String, String>` |  |
| fn | `take_logs` | `pub fn take_logs(&mut self) -> Vec<String>` |  |
| fn | `take_navigation` | `pub fn take_navigation(&mut self) -> Option<String>` |  |
| fn | `hit_test` | `pub fn hit_test(&self, x: f32, y: f32) -> Option<String>` |  |
| fn | `page_text` | `pub fn page_text(&self) -> String` |  |
| fn | `scroll_by` | `pub fn scroll_by(&mut self, dy: f32) -> Result<f32, String>` |  |
| fn | `fire_event` | `pub fn fire_event(&mut self, id: &str, ev: &str) -> Result<bool, String>` |  |
| fn | `run_timers` | `pub fn run_timers(&mut self, ms: u32) -> Result<usize, String>` |  |
| fn | `find_in_page` | `pub fn find_in_page(&mut self, q: &str) -> Result<Vec<(f32, f32, f32, f32)>, String>` |  |
| fn | `media_tick` | `pub fn media_tick(&mut self) -> Result<bool, String>` |  |
| fn | `media_click` | `pub fn media_click(&mut self, x: f32, y: f32) -> bool` |  |
| fn | `media_key` | `pub fn media_key(&mut self, k: &str) -> bool` |  |

Tests: `bad_input_is_an_error_never_a_panic`, `big_stack_survives_deep_recursion_that_small_stacks_cannot`, `codec_roundtrip_all_commands_and_replies`, `drop_joins_thread`, `host_runs_commands_on_engine_thread`, `panic_inside_job_is_contained_and_engine_recovers`, `remote_engine_is_a_drop_in_with_crash_state`, `timeout_reports_busy_and_host_survives`

### `src/ffi.rs`

C ABI for the shell (Go/cgo, C, C++). All functions are panic-safe: a panic never crosses FFI.
Strings returned are owned by the handle and valid until the next call on the same handle.
Build as cdylib (see PATCHES.md). Header: include/sparrow.h

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Handle` | `pub struct Handle` |  |
| fn | `sparrow_create` | `pub extern "C" fn sparrow_create() -> *mut Handle` |  |
| fn | `sparrow_destroy` | `pub unsafe extern "C" fn sparrow_destroy(p: *mut Handle)` |  |
| fn | `sparrow_load_url` | `pub unsafe extern "C" fn sparrow_load_url(p: *mut Handle, url: *const c_char) -> c_int` | 0 ok, 1 error (see sparrow_last_error), 2 page crashed (show "Page crashed, reload?", then sparrow_recover). |
| fn | `sparrow_load_html` | `pub unsafe extern "C" fn sparrow_load_html(p: *mut Handle, html: *const c_char, base: *const c_char) -> c_int` |  |
| fn | `sparrow_set_viewport` | `pub unsafe extern "C" fn sparrow_set_viewport(p: *mut Handle, w: u32, hh: u32) -> c_int` |  |
| fn | `sparrow_render` | `pub unsafe extern "C" fn sparrow_render(p: *mut Handle, out_w: *mut u32, out_h: *mut u32) -> *const u8` | Returns pointer to RGBA pixels (w*h*4) or null on error. Valid until next render/destroy. |
| fn | `sparrow_eval_js` | `pub unsafe extern "C" fn sparrow_eval_js(p: *mut Handle, code: *const c_char) -> *const c_char` | Returns the result string, or null on error (see sparrow_last_error). |
| fn | `sparrow_last_error` | `pub unsafe extern "C" fn sparrow_last_error(p: *mut Handle) -> *const c_char` |  |
| fn | `sparrow_title` | `pub unsafe extern "C" fn sparrow_title(p: *mut Handle) -> *const c_char` |  |
| fn | `sparrow_url` | `pub unsafe extern "C" fn sparrow_url(p: *mut Handle) -> *const c_char` |  |
| fn | `sparrow_take_logs` | `pub unsafe extern "C" fn sparrow_take_logs(p: *mut Handle) -> *const c_char` | console.log/alert lines since last call, joined by '\n'. |
| fn | `sparrow_take_navigation` | `pub unsafe extern "C" fn sparrow_take_navigation(p: *mut Handle) -> *const c_char` | URL JS asked to navigate to, or null. |
| fn | `sparrow_hit_test` | `pub unsafe extern "C" fn sparrow_hit_test(p: *mut Handle, x: f32, y: f32) -> *const c_char` | Link under viewport point (x, y), or null. |
| fn | `sparrow_scroll_by` | `pub unsafe extern "C" fn sparrow_scroll_by(p: *mut Handle, dy: f32) -> f32` | Returns new scroll_y (or -1.0 on error). |
| fn | `sparrow_fire_event` | `pub unsafe extern "C" fn sparrow_fire_event(p: *mut Handle, id: *const c_char, ev: *const c_char) -> c_int` |  |
| fn | `sparrow_run_timers` | `pub unsafe extern "C" fn sparrow_run_timers(p: *mut Handle, ms: u32) -> c_int` | Call every frame with elapsed ms. Returns callbacks run, or -1 on error. |
| fn | `sparrow_find` | `pub unsafe extern "C" fn sparrow_find(p: *mut Handle, q: *const c_char) -> *const c_char` | "x,y,w,h;x,y,w,h;..." in document coordinates. |
| fn | `sparrow_page_text` | `pub unsafe extern "C" fn sparrow_page_text(p: *mut Handle) -> *const c_char` |  |
| fn | `sparrow_is_crashed` | `pub unsafe extern "C" fn sparrow_is_crashed(p: *mut Handle) -> c_int` |  |
| fn | `sparrow_recover` | `pub unsafe extern "C" fn sparrow_recover(p: *mut Handle)` |  |

### `src/forms.rs`

HTML form submission: collect successful controls (HTML spec 4.10.21), urlencode, build GET/POST request.
Pure logic over `Dom`; the network call itself is done by `dro_net` (needs a POST helper: see FormRequest).

| Kind | Name | Signature | Doc |
|---|---|---|---|
| enum | `Method` | `pub enum Method` |  |
| struct | `FormRequest` | `pub struct FormRequest` |  |
| fn | `urlencode` | `pub fn urlencode(s: &str) -> String` | application/x-www-form-urlencoded byte serializer. |
| fn | `urldecode` | `pub fn urldecode(s: &str) -> String` |  |
| fn | `encode_pairs` | `pub fn encode_pairs(p: &[(String, String)]) -> String` |  |
| fn | `find_form` | `pub fn find_form(dom: &Dom, node: NodeId) -> Option<NodeId>` | Closest ancestor `<form>` of `node` (or the node itself). |
| fn | `collect` | `pub fn collect(dom: &Dom, form: NodeId, submitter: Option<NodeId>) -> Vec<(String, String)>` | Successful controls of `form` in tree order. `submitter` is the button that was clicked (if any). |
| fn | `build_request` | `pub fn build_request(dom: &Dom, form: NodeId, submitter: Option<NodeId>, base: &str) -> Result<FormRequest, String>` | Build the request for submitting `form` (optionally via `submitter`) on a page at `base`. |

Tests: `collects_successful_controls_in_order`, `empty_action_uses_page_url_and_find_form`, `encoding_roundtrip`, `get_replaces_query_and_drops_fragment`, `post_and_overrides_and_errors`, `select_defaults_to_first_option`

### `src/input.rs`

Text editing model for <input>/<textarea>: caret, selection, typing, delete, word/line movement,
clipboard, undo/redo, mouse mapping. DOM-independent: the engine owns one `TextField` per focused control.
Caret moves by grapheme-ish clusters (Bengali/Devanagari vowel signs + virama conjuncts stay together);
Backspace/Delete remove one code point (like most editors).

| Kind | Name | Signature | Doc |
|---|---|---|---|
| enum | `Key` | `pub enum Key` |  |
| struct | `Mods` | `pub struct Mods` |  |
| trait | `Clipboard` | `pub trait Clipboard` |  |
| struct | `MemoryClipboard` | `pub struct MemoryClipboard(pub String)` |  |
| enum | `Outcome` | `pub enum Outcome` |  |
| struct | `TextField` | `pub struct TextField` |  |
| fn | `new` | `pub fn new(multiline: bool) -> Self` |  |
| fn | `with_text` | `pub fn with_text(multiline: bool, t: &str) -> Self` |  |
| fn | `set_text` | `pub fn set_text(&mut self, t: &str)` | Replace the whole value (programmatic; clears undo history). Caret goes to the end. |
| fn | `text` | `pub fn text(&self) -> String` |  |
| fn | `display_text` | `pub fn display_text(&self) -> String` | What to draw (password fields show '*'). |
| fn | `len` | `pub fn len(&self) -> usize` |  |
| fn | `is_empty` | `pub fn is_empty(&self) -> bool` |  |
| fn | `caret` | `pub fn caret(&self) -> usize` |  |
| fn | `selection` | `pub fn selection(&self) -> Option<(usize, usize)>` |  |
| fn | `selected_text` | `pub fn selected_text(&self) -> String` |  |
| fn | `lines` | `pub fn lines(&self) -> Vec<(usize, usize)>` |  |
| fn | `undo` | `pub fn undo(&mut self) -> bool` |  |
| fn | `redo` | `pub fn redo(&mut self) -> bool` |  |
| fn | `insert_str` | `pub fn insert_str(&mut self, s: &str) -> bool` | Insert text at the caret (typing, paste, IME commit). Respects `max_len` and single-line rules. |
| fn | `select_all` | `pub fn select_all(&mut self)` |  |
| fn | `set_caret` | `pub fn set_caret(&mut self, idx: usize, extend: bool)` | Set caret from the mouse (`extend` = shift/drag). |
| fn | `select_word` | `pub fn select_word(&mut self, idx: usize)` | Double-click: select the word under `idx`. |
| fn | `handle_key` | `pub fn handle_key(&mut self, key: Key, m: Mods, clip: &mut dyn Clipboard) -> Outcome` |  |
| fn | `x_of` | `pub fn x_of(&self, idx: usize, measure: &dyn Fn(&str) -> f32) -> f32` | X offset of caret index `idx` inside its line, using the shaper's `measure`. |
| fn | `index_at` | `pub fn index_at(&self, x: f32, line: usize, measure: &dyn Fn(&str) -> f32) -> usize` | Nearest caret index for a click at `x` on line `line` (0-based). |
| fn | `line_of` | `pub fn line_of(&self, idx: usize) -> usize` |  |

Tests: `bangla_clusters_move_together_backspace_by_codepoint`, `clipboard_cut_copy_paste_and_single_line_filter`, `mouse_mapping_and_password`, `multiline_up_down_keeps_column_and_enter`, `selection_replace_and_shift_arrows`, `single_line_enter_submits_and_max_len`, `typing_backspace_delete`, `undo_redo_coalesces_typing`, `word_movement_and_delete`

### `src/js_engine.rs`

JS engine abstraction: DOM/bridge code should depend on `JsEngine`, never on boa directly.
Swap Boa -> QuickJS -> V8 by adding one `impl JsEngine` and one arm in `create()`.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| trait | `JsEngine` | `pub trait JsEngine` |  |
| struct | `NullJs` | `pub struct NullJs` | JavaScript disabled (settings: "JS off"). |
| fn | `create` | `pub fn create(kind: &str) -> Result<Box<dyn JsEngine>, String>` | kind: "boa" (default) \| "off". Add "quickjs" / "v8" here later. |

Tests: `boa_evaluates`, `off_engine_refuses`, `unknown_engine_errors`

### `src/lib.rs`

Sparrow Engine: Rust browser engine embedded in the Sparrow shell.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| mod | `dro_charset` | `pub mod dro_charset` |  |
| mod | `dro_css` | `pub mod dro_css` |  |
| mod | `dro_css_ext` | `pub mod dro_css_ext` |  |
| mod | `dro_dom` | `pub mod dro_dom` |  |
| mod | `dro_dom_js` | `pub mod dro_dom_js` |  |
| mod | `dro_html` | `pub mod dro_html` |  |
| mod | `dro_layout` | `pub mod dro_layout` |  |
| mod | `dro_paint` | `pub mod dro_paint` |  |
| mod | `dro_js` | `pub mod dro_js` |  |
| mod | `js_engine` | `pub mod js_engine` |  |
| mod | `dro_blocklist` | `pub mod dro_blocklist` |  |
| mod | `dro_font` | `pub mod dro_font` |  |
| mod | `dro_media` | `pub mod dro_media` |  |
| mod | `dro_media_ui` | `pub mod dro_media_ui` |  |
| mod | `dro_net` | `pub mod dro_net` |  |
| mod | `browser` | `pub mod browser` |  |
| mod | `ffi` | `pub mod ffi` |  |
| mod | `safety` | `pub mod safety` |  |
| mod | `sparrow_engine` | `pub mod sparrow_engine` |  |
| fn | `create_engine` | `pub fn create_engine() -> SparrowEngine` |  |

### `src/safety.rs`

Crash/hang protection helpers (std only).
NOTE: a real stack overflow or OOM still kills the process; big stacks make it rare,
process isolation (later) removes it.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `log` | `pub fn log(msg: impl Into<String>)` | RAM-only ring log (never touches disk). |
| fn | `recent_logs` | `pub fn recent_logs() -> Vec<String>` |  |
| fn | `clear_logs` | `pub fn clear_logs()` |  |
| fn | `install_panic_hook` | `pub fn install_panic_hook()` | Panics are written to the RAM log instead of stderr/disk. |
| fn | `run_guarded` | `pub fn run_guarded<T>(label: &str, f: impl FnOnce() -> T + UnwindSafe) -> Result<T, String>` | Run `f`, turning a panic into `Err`. |
| fn | `run_big_stack` | `pub fn run_big_stack<T: Send + 'static>(stack_mb: usize, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String>` | Run `f` on a fresh thread with a large stack (deep DOM/CSS recursion). |
| fn | `run_with_timeout` | `pub fn run_with_timeout<T: Send + 'static>(ms: u64, stack_mb: usize, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String>` | Like `run_big_stack` but gives up after `ms`. A stuck thread cannot be killed in safe Rust: it is abandoned; after MAX_ABANDONED stuck threa |
| fn | `abandoned_workers` | `pub fn abandoned_workers() -> usize` |  |
| struct | `Deadline` | `pub struct Deadline` | Cooperative deadline for long Rust loops (layout, parsing): call `expired()` every N iterations. |
| fn | `after_ms` | `pub fn after_ms(ms: u64) -> Self` |  |
| fn | `expired` | `pub fn expired(&self) -> bool` |  |
| fn | `check` | `pub fn check(&self, what: &str) -> Result<(), String>` |  |
| mod | `limits` | `pub mod limits` | Hard limits used by the engine (keep in one place). |
| const | `MAX_DOM_NODES` | `pub const MAX_DOM_NODES: usize = 200_000` |  |
| const | `MAX_DOM_DEPTH` | `pub const MAX_DOM_DEPTH: usize = 512` |  |
| const | `MAX_CSS_BYTES` | `pub const MAX_CSS_BYTES: usize = 2 * 1024 * 1024` |  |
| const | `MAX_SCRIPT_BYTES` | `pub const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024` |  |
| const | `MAX_PIXELS` | `pub const MAX_PIXELS: u64 = 16_000_000; // 4000x4000 RGBA = 64 MB` |  |
| const | `MAX_SIDE` | `pub const MAX_SIDE: u32 = 16_384` |  |
| fn | `viewport_ok` | `pub fn viewport_ok(w: u32, h: u32) -> Result<(), String>` |  |

Tests: `big_stack_deep_recursion`, `deadline_and_limits`, `guarded_catches_panic`, `log_ring_is_bounded`, `timeout_returns`

### `src/sparrow_engine.rs`

SparrowEngine: one page = DOM + styles + layout + JS + media. Everything is RAM-only.
Child modules: `bridge` (page install, JS DOM, events, timers, find) and `media_bridge` (<video>/<audio>).

| Kind | Name | Signature | Doc |
|---|---|---|---|
| const | `CRASH_MSG` | `pub const CRASH_MSG: &str = "Page crashed, reload?"` |  |
| struct | `SparrowEngine` | `pub struct SparrowEngine` |  |
| fn | `new` | `pub fn new() -> Self` |  |
| fn | `is_crashed` | `pub fn is_crashed(&self) -> bool` |  |
| fn | `recover` | `pub fn recover(&mut self)` |  |
| fn | `url` | `pub fn url(&self) -> &str` |  |
| fn | `title` | `pub fn title(&self) -> &str` |  |
| fn | `status` | `pub fn status(&self) -> u16` | HTTP status of the last main-page load (0 = none). |
| fn | `viewport` | `pub fn viewport(&self) -> (u32, u32)` |  |
| fn | `set_viewport` | `pub fn set_viewport(&mut self, w: u32, h: u32)` | Re-styles for the new size (media queries, vw/vh). |
| fn | `net_stats` | `pub fn net_stats(&self) -> dro_net::Stats` | Network counters (requests, cache hits, blocked trackers, ...). |
| fn | `clear_session_data` | `pub fn clear_session_data(&mut self)` | "Clear browsing data": cookies, HTTP cache, HSTS (all RAM) + this page's images/JS storage. |
| fn | `reload` | `pub fn reload(&mut self) -> Result<(), String>` |  |
| fn | `load_url` | `pub fn load_url(&mut self, input: &str) -> Result<(), String>` |  |
| fn | `load_html` | `pub fn load_html(&mut self, html: &str, base: &str) -> Result<(), String>` | Load raw HTML (no network for the page itself); sub-resources resolve against `base`. |
| fn | `render` | `pub fn render(&mut self) -> Result<Vec<u8>, String>` | Returns RGBA pixels (viewport width * height * 4); media control bars are overlaid. |
| fn | `eval_js` | `pub fn eval_js(&mut self, code: &str) -> Result<String, String>` |  |
| fn | `take_logs` | `pub fn take_logs(&mut self) -> Vec<String>` | console.log / alert output since last call |
| fn | `scroll_by` | `pub fn scroll_by(&mut self, dy: f32) -> Result<f32, String>` |  |
| fn | `hit_test` | `pub fn hit_test(&self, x: f32, y: f32) -> Option<String>` | Click at viewport (x, y). Returns resolved link URL if a link was hit. (Shell: call `media_click` first; if it returns true, skip this.) |

Tests: `blank_engine_state`, `escapes_html`, `load_html_sets_title_and_renders`, `recover_keeps_viewport_and_url`, `same_doc_needs_loaded_page`

### `src/sparrow_engine/bridge.rs`

Child module of `sparrow_engine` (so it can touch private fields).
Adds: install_page (JS DOM + charset + css preprocessor), restyle, events, timers,
find-in-page, page text, DOM dump. Does not change existing public API.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| fn | `restyle` | `pub(super) fn restyle(&mut self)` | Rebuild computed styles from UA + fetched <link> CSS + current <style> elements. Called after load, after viewport change (media queries/vw/ |
| fn | `install_page` | `pub(super) fn install_page(&mut self, html: &str, base: &str) -> Result<(), String>` | Full page install. Replaces the old `install` (call sites: load_url, load_html). |
| fn | `apply_js` | `pub(super) fn apply_js(&mut self) -> Result<(), String>` | Pull navigation + DOM changes out of JS and rebuild styles if needed. |
| fn | `take_navigation` | `pub fn take_navigation(&mut self) -> Option<String>` | URL requested by page JS (link click handled in JS, `location.href=`). Shell should `load_url` it. |
| fn | `fire_event` | `pub fn fire_event(&mut self, element_id: &str, event: &str) -> Result<bool, String>` | Dispatch a DOM event (e.g. "click") on the element whose `id` attribute is `element_id`. |
| fn | `run_timers` | `pub fn run_timers(&mut self, elapsed_ms: u32) -> Result<usize, String>` | Advance JS timers by `elapsed_ms` (call ~every frame / 16 ms). Returns number of callbacks run. |
| fn | `resize` | `pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String>` | Re-run styling for a new viewport (media queries, vw/vh). Replaces the old `set_viewport` body. |
| fn | `document_height` | `pub fn document_height(&mut self) -> Result<f32, String>` |  |
| fn | `scroll_y` | `pub fn scroll_y(&self) -> f32` |  |
| fn | `find_in_page` | `pub fn find_in_page(&mut self, query: &str) -> Result<Vec<(f32, f32, f32, f32)>, String>` | Find-in-page: rectangles `(x, y, w, h)` in DOCUMENT coordinates (subtract `scroll_y()` to draw). Multi-word queries match consecutive words. |
| fn | `page_text` | `pub fn page_text(&self) -> String` | Visible text of the page (reader-mode style): scripts/styles skipped, blocks separated by newlines. |
| fn | `dump_dom` | `pub fn dump_dom(&self) -> String` | Debug tree dump (devtools-lite). Depth capped at 40, text truncated. |

### `src/sparrow_engine/media_bridge.rs`

Child module of `sparrow_engine`: connects <video>/<audio> elements to `dro_media::Media`.
Fetch happens on a worker thread (bytes only); `Media` (cpal stream is !Send) is built on the engine thread.
Video frames are injected into `self.images` under the key "media:<index>", so the existing
image painter draws them; the control bar is overlaid in `overlay_media`.

| Kind | Name | Signature | Doc |
|---|---|---|---|
| struct | `Slot` | `pub(super) struct Slot` |  |
| fn | `setup_media` | `pub(super) fn setup_media(&mut self, base: &str)` | Create one slot per <video>/<audio> (document order == JS `mediaList()` order) and start fetching. |
| fn | `media_tick` | `pub fn media_tick(&mut self) -> Result<bool, String>` | Drive media: finish downloads, run JS commands, upload frames, fire events. Call every frame (~16 ms). Returns true when the picture changed |
| fn | `overlay_media` | `pub(super) fn overlay_media(&mut self, px: Vec<u8>) -> Vec<u8>` | Draw control bars on top of a rendered frame (viewport pixels). No media -> returns input untouched. |
| fn | `media_click` | `pub fn media_click(&mut self, x: f32, y: f32) -> bool` | Click in viewport coordinates. Returns true if a media control handled it (then skip link hit-test). |
| fn | `media_key` | `pub fn media_key(&mut self, key: &str) -> bool` | Keyboard for the first loaded media: " " play/pause, ArrowLeft/Right +-5s, ArrowUp/Down volume. |

### `src/dom_bootstrap.js`

JS-side DOM shim loaded into Boa. Globals it installs:

- `CustomEvent`
- `Element`
- `Event`
- `HTMLElement`
- `IntersectionObserver`
- `MutationObserver`
- `Node`
- `ResizeObserver`
- `Text`
- `XMLHttpRequest`
- `__DOM`
- `__fire`
- `__fireOn`
- `__isDirty`
- `__mediaCmds`
- `__mediaEvent`
- `__mediaState`
- `__runTimers`
- `__serialize`
- `__takeNav`
- `_ls`
- `addEventListener`
- `alert`
- `blur`
- `cancelAnimationFrame`
- `clearInterval`
- `clearTimeout`
- `confirm`
- `devicePixelRatio`
- `dispatchEvent`
- `document`
- `fetch`
- `focus`
- `getComputedStyle`
- `globalThis`
- `history`
- `innerHeight`
- `innerWidth`
- `localStorage`
- `location`
- `matchMedia`
- `navigator`
- `open`
- `outerHeight`
- `outerWidth`
- `parent`
- `parentNode`
- `performance`
- `prompt`
- `queueMicrotask`
- `removeEventListener`
- `requestAnimationFrame`
- `scroll`
- `scrollBy`
- `scrollTo`
- `self`
- `sessionStorage`
- `setInterval`
- `setTimeout`
- `top`
- `window`

---

## C ABI reference

Header: `include/sparrow.h`. Return codes: `0` ok, `1` error (see `sparrow_last_error`), `2` page crashed.

| Function | Signature | Doc |
|---|---|---|
| `sparrow_create` | `() -> *mut Handle` |  |
| `sparrow_destroy` | `(p: *mut Handle) -> ()` |  |
| `sparrow_load_url` | `(p: *mut Handle, url: *const c_char) -> c_int` | 0 ok, 1 error (see sparrow_last_error), 2 page crashed (show "Page crashed, reload?", then sparrow_recover). |
| `sparrow_load_html` | `(p: *mut Handle, html: *const c_char, base: *const c_char) -> c_int` |  |
| `sparrow_set_viewport` | `(p: *mut Handle, w: u32, hh: u32) -> c_int` |  |
| `sparrow_render` | `(p: *mut Handle, out_w: *mut u32, out_h: *mut u32) -> *const u8` | Returns pointer to RGBA pixels (w*h*4) or null on error. Valid until next render/destroy. |
| `sparrow_eval_js` | `(p: *mut Handle, code: *const c_char) -> *const c_char` | Returns the result string, or null on error (see sparrow_last_error). |
| `sparrow_last_error` | `(p: *mut Handle) -> *const c_char` |  |
| `sparrow_title` | `(p: *mut Handle) -> *const c_char` |  |
| `sparrow_url` | `(p: *mut Handle) -> *const c_char` |  |
| `sparrow_take_logs` | `(p: *mut Handle) -> *const c_char` | console.log/alert lines since last call, joined by '\n'. |
| `sparrow_take_navigation` | `(p: *mut Handle) -> *const c_char` | URL JS asked to navigate to, or null. |
| `sparrow_hit_test` | `(p: *mut Handle, x: f32, y: f32) -> *const c_char` | Link under viewport point (x, y), or null. |
| `sparrow_scroll_by` | `(p: *mut Handle, dy: f32) -> f32` | Returns new scroll_y (or -1.0 on error). |
| `sparrow_fire_event` | `(p: *mut Handle, id: *const c_char, ev: *const c_char) -> c_int` |  |
| `sparrow_run_timers` | `(p: *mut Handle, ms: u32) -> c_int` | Call every frame with elapsed ms. Returns callbacks run, or -1 on error. |
| `sparrow_find` | `(p: *mut Handle, q: *const c_char) -> *const c_char` | "x,y,w,h;x,y,w,h;..." in document coordinates. |
| `sparrow_page_text` | `(p: *mut Handle) -> *const c_char` |  |
| `sparrow_is_crashed` | `(p: *mut Handle) -> c_int` |  |
| `sparrow_recover` | `(p: *mut Handle) -> ()` |  |

---

## Shell integration (Go/cgo)

1. Build the engine as a C library:
   - `cargo build --release --target x86_64-pc-windows-gnu` (MinGW, same toolchain as the Sparrow shell)
   - outputs `sparrow_engine.dll` (+ `libsparrow_engine.dll.a` import lib). Copy next to `sparrow.h`.
2. Threading: **one engine = one OS thread.** Call every function of a handle from the same worker goroutine
   (`runtime.LockOSThread()`), never from the UI thread: `load_url` blocks on the network.
3. Frame loop: `run_timers(16)` -> `take_navigation()` -> `render()` -> blit RGBA to the WebView/canvas.
4. Crash UI: any call returning `2` (or `is_crashed()==1`) -> show "Page crashed, reload?"; on click `sparrow_recover()` then `sparrow_load_url(url)`.

```go
package sparrow

/*
#cgo windows LDFLAGS: -L${SRCDIR}/lib -lsparrow_engine
#include <stdlib.h>
#include "sparrow.h"
*/
import "C"
import (
	"errors"
	"unsafe"
)

var ErrCrashed = errors.New("Page crashed, reload?")

type Engine struct{ h *C.SparrowHandle }

func New() *Engine  { return &Engine{h: C.sparrow_create()} }
func (e *Engine) Close() { C.sparrow_destroy(e.h) }

func (e *Engine) lastErr() error { return errors.New(C.GoString(C.sparrow_last_error(e.h))) }

func (e *Engine) Load(url string) error {
	cs := C.CString(url)
	defer C.free(unsafe.Pointer(cs))
	switch C.sparrow_load_url(e.h, cs) {
	case 0:
		return nil
	case 2:
		return ErrCrashed
	}
	return e.lastErr()
}

func (e *Engine) Resize(w, h uint32) { C.sparrow_set_viewport(e.h, C.uint32_t(w), C.uint32_t(h)) }

// Render returns a copy of the RGBA frame.
func (e *Engine) Render() ([]byte, int, int, error) {
	var w, h C.uint32_t
	p := C.sparrow_render(e.h, &w, &h)
	if p == nil {
		if C.sparrow_is_crashed(e.h) == 1 {
			return nil, 0, 0, ErrCrashed
		}
		return nil, 0, 0, e.lastErr()
	}
	return C.GoBytes(unsafe.Pointer(p), C.int(w*h*4)), int(w), int(h), nil
}

func (e *Engine) Tick(ms uint32) (nav string) {
	C.sparrow_run_timers(e.h, C.uint32_t(ms))
	if n := C.sparrow_take_navigation(e.h); n != nil {
		return C.GoString(n)
	}
	return ""
}

func (e *Engine) Recover() { C.sparrow_recover(e.h) }

---

## Phase 4 integration guide

Phase-4 files are present but **not compiled** until these edits are applied.

| File | Edit |
|---|---|
| `src/lib.rs` | add `pub mod dro_cache; pub mod engine_host; pub mod forms; pub mod input;` |
| `src/browser/mod.rs` | add `pub mod store;` |
| `Cargo.toml` | delete the `rusqlite` line; add `unicode-bidi = "0.3"` and `unicode-linebreak = "0.1"` |
| `src/ffi.rs` | `use crate::engine_host::RemoteEngine;`, field `engine: RemoteEngine`, constructor `RemoteEngine::new()` |
| `src/browser/tabs.rs` | use `RemoteEngine` instead of `SparrowEngine` |
| `browser/history.rs`, `session.rs`, `bookmark.rs`, `settings.rs` | replace with the RAM-only / `store.rs` versions |
| `Bookmarks::add/remove`, `Settings::set/remove`, `History::add/clear` | now take `&mut self` |

### Not wired yet

- **Forms:** on submit click call `forms::find_form` then `forms::build_request`; send POST through `dro_net::post_form`.
- **Cache:** in the fetch path call `Cache::lookup` first; on `Stale` send `If-None-Match` / `If-Modified-Since`; on 304 call `refresh`; on 200 call `store`. One `Cache` per engine. `dro_net` has its own cache today and ignores `Vary`.
- **Text input:** engine keeps `Option<(NodeId, TextField)>` for the focused control; key events go to `TextField::handle_key`; on `Outcome::Changed` write `text()` back to the `value` attribute and mark layout dirty; draw caret and selection with `x_of` / `index_at` and `dro_font::measure`.
- **Process isolation:** `Cmd`/`Reply` already have a binary codec; a `ProcessHost` only spawns a child and pipes `Cmd::to_bytes()` / `Reply::from_bytes()` frames with a `u32` length prefix.

### Fixes already applied to Phase 4

- `engine_host.rs`: `RemoteEngine::fin()` keeps the crash flag after `sync()` (the host recovers the engine after a panic, which used to reset the flag and make ffi return 1 instead of 2).
- `input.rs`: a virama no longer glues a following newline, so the caret cannot jump a line.
- `forms.rs`: a control inside a form but owned by another form via `form="id"` is no longer submitted.

---

## Storage and privacy model

| Data | Where | Lifetime |
|---|---|---|
| Bookmarks | `store.rs` text file (`key<TAB>value`, escaped, atomic tmp+rename) | persistent |
| Settings | second `store.rs` file (use a different path) | persistent |
| History | RAM (`history.rs`, max 5000 entries) | process |
| Recently closed tabs | RAM (`session.rs`, max 25) | process |
| HTTP cache | RAM (`dro_net` cache; `dro_cache` when wired) | process |
| Cookies, HSTS | RAM (`RamJar`) | process; export/import available |
| Page images, JS storage | RAM per engine | page |

Blocklist: third-party trackers are dropped before the request. `Do Not Track` is sent when enabled in `Config`.

---

## Threading and crash model

- One engine = one OS thread. `boa` `Context` is `!Send`, so the engine is created on its own thread and never leaves it.
- `EngineHost::call(closure)` runs code on that thread; panics are caught, the engine is recovered, the caller gets `"Page crashed, reload?"`.
- `call_timeout` returns `"engine busy (timeout)"` but the job keeps running; a hung JS loop is not interruptible in Boa.
- Default stack is 256 MB of reserved address space (committed lazily); falls back to 64 MB then 16 MB.
- A stack overflow, abort or OOM still kills the process. Only a child process (planned) survives those.
- `SparrowEngine::guard` also catches panics inside the engine itself and sets `crashed`; every load method auto-recovers a crashed engine.
- `ffi.rs` wraps every export in `catch_unwind`; no panic crosses the C boundary.

---

## Security model

| Control | State |
|---|---|
| Panic isolation | done |
| Viewport and DOM size limits | done (`safety::limits`) |
| Script size limit, loop and recursion limits | done |
| Private-network sub-resource and redirect block | done (not DNS-rebinding safe) |
| Third-party tracker blocking | done |
| https-first, HSTS | done |
| Only classic `<script>` types run | done |
| Same-origin policy | missing |
| CORS | missing |
| CSP | missing |
| Mixed-content blocking | missing |
| SameSite cookies | missing |
| JS sandbox beyond Boa limits | missing |
| Process isolation | missing |

---

## Testing

```text
cargo test                 # unit + integration
cargo test --test smoke    # tests/smoke.rs
cargo test --test fixes    # tests/fixes.rs
cargo clippy -- -D warnings
```

Phase-4 unit tests (38) were last run against a stub engine with the same signatures, not the real crate.

### Test index

| File | Tests |
|---|---|
| `src/browser/history.rs` | add_search_clear, ignores_blank_and_caps |
| `src/browser/store.rs` | corrupt_lines_skipped_and_prefixes_isolated, memory_mode_never_touches_disk, roundtrip_with_special_chars, settings_defaults_and_remove |
| `src/dro_blocklist.rs` | blocks_third_party_trackers, first_party_never_blocked, list_formats |
| `src/dro_cache.rs` | fresh_then_stale_then_refresh, lru_eviction_by_bytes_and_entry_cap, parses_policy, policy_rules, touch_protects_recent_and_replace_updates_size |
| `src/dro_charset.rs` | decode_bom_and_utf8, decode_latin1_and_cp1252, header_charset, meta_charset |
| `src/dro_css_ext.rs` | at_rules_dropped, calc_eval, media_filtering, media_queries, units, vars |
| `src/dro_dom_js.rs` | escapes, literal_shape |
| `src/dro_font.rs` | bidi_fast_path_and_rtl, caret_stops_are_monotonic_and_cluster_aware, css_family_parsing, index_at_x_picks_nearest, latin_and_bangla_shape, lazy_loading_latin_loads_few_faces, line_break_opportunities, mask_raster_and_blit, renders_pixels, rtl_shaping_does_not_panic, style_builder_clamps, styles_change_width_and_cache_hits, synthetic_italic_shifts_top, web_font_rejects_garbage_and_woff |
| `src/dro_media.rs` | bad_input_is_error, channel_conversion, resampler_ratio, wall_clock |
| `src/dro_media_ui.rs` | draws_without_panic, hits, time_format |
| `src/dro_net.rs` | cache_fresh_and_stale, cookie_basic_and_security, cookie_export_import_roundtrip, cookie_rejections_and_delete, data_urls_and_base64, domains, hsts_rules, http_dates, par_map_keeps_order, private_hosts, referrer_policy, sniff_and_decode, third_party_cookie_blocking |
| `src/engine_host.rs` | bad_input_is_an_error_never_a_panic, big_stack_survives_deep_recursion_that_small_stacks_cannot, codec_roundtrip_all_commands_and_replies, drop_joins_thread, host_runs_commands_on_engine_thread, panic_inside_job_is_contained_and_engine_recovers, remote_engine_is_a_drop_in_with_crash_state, timeout_reports_busy_and_host_survives |
| `src/forms.rs` | collects_successful_controls_in_order, empty_action_uses_page_url_and_find_form, encoding_roundtrip, get_replaces_query_and_drops_fragment, post_and_overrides_and_errors, select_defaults_to_first_option |
| `src/input.rs` | bangla_clusters_move_together_backspace_by_codepoint, clipboard_cut_copy_paste_and_single_line_filter, mouse_mapping_and_password, multiline_up_down_keeps_column_and_enter, selection_replace_and_shift_arrows, single_line_enter_submits_and_max_len, typing_backspace_delete, undo_redo_coalesces_typing, word_movement_and_delete |
| `src/js_engine.rs` | boa_evaluates, off_engine_refuses, unknown_engine_errors |
| `src/safety.rs` | big_stack_deep_recursion, deadline_and_limits, guarded_catches_panic, log_ring_is_bounded, timeout_returns |
| `src/sparrow_engine.rs` | blank_engine_state, escapes_html, load_html_sets_title_and_renders, recover_keeps_viewport_and_url, same_doc_needs_loaded_page |
| `tests/fixes.rs` | bangla_fragment_scrolls_to_anchor, console_log_is_bounded, huge_viewport_is_rejected_not_allocated, image_without_data_shows_alt_text_and_renders, non_js_scripts_are_not_evaluated |
| `tests/regression.rs` | all |
| `tests/smoke.rs` | deep_nesting_does_not_kill_process, garbage_html_survives, history_is_ram_only, js_eval_works, loads_and_renders_pixels, recover_after_crash_flag, scroll_and_hit_test_never_panic |

Total named tests found: **109**.

---

## Build blockers

- `unicode-bidi` and `unicode-linebreak` are used by `dro_font.rs` but missing from `Cargo.toml`.
- Phase-4 modules are not declared in `lib.rs`.
- `rusqlite (bundled)` is still listed although Phase 4 removed SQLite.
- If cargo is older than 1.80: `cargo update -p url --precise 2.5.0` and `cargo update -p encoding_rs --precise 0.8.34`.
- Linux needs `libasound2-dev` for cpal.

---

## Not covered yet

### JavaScript

- `fetch` / `XMLHttpRequest` (stubs reject)
- ES modules, external `async` / `defer` semantics
- interrupting an infinite loop inside `try/catch`

### Networking and security

- same-origin, CORS, CSP, mixed content
- SameSite cookie enforcement
- cache `Vary` handling
- DNS-rebinding protection
- process-global `dro_net::set_page` (wrong with several tabs)

### Layout and paint

- `white-space: pre` (plain text and JSON lose line breaks)
- scroll containers, `overflow`
- incremental style/layout/paint, tile cache
- GPU rendering

### Media

- video frame decoding (only GIF and audio)
- opus, wma
- streaming, HLS, DASH, range playback
- fullscreen, subtitles, playback rate

### Input

- focus management, tab order
- caret and selection drawing
- IME composition
- clipboard bridge to the OS

### Platform

- process isolation
- scheduler with frame budget and priorities
- requestAnimationFrame
- web fonts loading from `@font-face` end to end

---

## Roadmap

| Step | Goal | Exit check |
|---|---|---|
| 0 | Build is green | `cargo build` and `cargo test` pass |
| 1 | Wire Phase 4 | modules declared, `RemoteEngine` in ffi/tabs, SQLite removed |
| 2 | Forms + input + cache wired | submit works; typing in `<input>`; 304 revalidation |
| 3 | `origin.rs` | same-origin, CORS, mixed-content, CSP tests |
| 4 | `js_fetch.rs` | `fetch()` returns text/json through the origin checks |
| 5 | `scheduler.rs` | 16 ms frame budget, rAF, per-tab page context |
| 6 | `invalidate.rs` + `raster_cache.rs` | tile output equals full paint |
| 7 | `process_host.rs` | child crash does not kill the shell |
| 8 | Video frames | AV1 or platform decoder for MP4/WebM |

---

## Engineering rules

- No `unwrap`/`expect` outside tests; return `Result<_, String>`.
- Never write to disk except bookmarks and settings.
- Every new public method on the engine goes through `guard()`.
- Every C export goes through `guard()` in `ffi.rs` and returns a code, never unwinds.
- Keep files small; one concern per file; unit tests next to code.
- Do not claim a feature works until a test proves it against the real crate.
- Update `README.md` and `phases.md` with verified behaviour only.

---

## Glossary

- **Arena DOM**: nodes stored in a `Vec`, referenced by index (`NodeId`)
- **Cascade**: CSS rule resolution into a computed `Style` per node
- **Shaping**: turning text into positioned glyphs (rustybuzz)
- **Bidi**: bidirectional text ordering (Arabic/Hebrew with Latin)
- **Conjunct**: Bangla/Indic consonant cluster joined by a virama
- **Virama**: combining mark that suppresses the inherent vowel (U+09CD for Bangla)
- **Damage rect**: screen area that must be repainted
- **CORS**: cross-origin resource sharing
- **CSP**: content security policy
- **HSTS**: forced HTTPS for a host
- **RGBA frame**: premultiplied 8-bit pixels, `w * h * 4` bytes
- **Handle**: opaque pointer the shell holds for one engine

---

_Author: Kaif Alvi (Codone). Shell: Go + WebView. Engine: Rust._

## Appendix: public function index

- `abandoned_workers` in `src/safety.rs`
- `active` in `src/browser/tabs.rs`
- `add` in `src/browser/bookmark.rs`
- `add` in `src/browser/history.rs`
- `add` in `src/browser/store.rs`
- `add_rule` in `src/dro_blocklist.rs`
- `advance` in `src/dro_paint.rs`
- `after_ms` in `src/safety.rs`
- `append_child` in `src/dro_dom.rs`
- `apply_js` in `src/sparrow_engine/bridge.rs`
- `attr` in `src/dro_dom.rs`
- `available` in `src/dro_font.rs`
- `bidi_runs` in `src/dro_font.rs`
- `bold` in `src/dro_font.rs`
- `build_request` in `src/forms.rs`
- `bytes` in `src/dro_cache.rs`
- `cache_bytes` in `src/dro_net.rs`
- `cache_clear` in `src/dro_net.rs`
- `call` in `src/engine_host.rs`
- `call_timeout` in `src/engine_host.rs`
- `caret` in `src/input.rs`
- `caret_stops` in `src/dro_font.rs`
- `caret_stops` in `src/dro_font.rs`
- `cells` in `src/dro_layout.rs`
- `charset_from_header` in `src/dro_charset.rs`
- `charset_from_meta` in `src/dro_charset.rs`
- `check` in `src/safety.rs`
- `clear` in `src/browser/history.rs`
- `clear` in `src/browser/session.rs`
- `clear` in `src/dro_cache.rs`
- `clear` in `src/dro_net.rs`
- `clear_all` in `src/dro_net.rs`
- `clear_host` in `src/dro_net.rs`
- `clear_logs` in `src/safety.rs`
- `clear_session_data` in `src/sparrow_engine.rs`
- `clear_web_fonts` in `src/dro_font.rs`
- `clear_web_fonts` in `src/dro_font.rs`
- `close` in `src/browser/tabs.rs`
- `collect` in `src/forms.rs`
- `compute` in `src/dro_css.rs`
- `compute` in `src/dro_layout.rs`
- `config` in `src/dro_net.rs`
- `contains` in `src/browser/bookmark.rs`
- `contains` in `src/browser/store.rs`
- `cookie_count` in `src/dro_net.rs`
- `cookies_clear` in `src/dro_net.rs`
- `cookies_clear_host` in `src/dro_net.rs`
- `cookies_export` in `src/dro_net.rs`
- `cookies_import` in `src/dro_net.rs`
- `count` in `src/dro_font.rs`
- `create` in `src/dro_dom.rs`
- `create` in `src/js_engine.rs`
- `create_engine` in `src/lib.rs`
- `current_time` in `src/dro_media.rs`
- `decode` in `src/dro_charset.rs`
- `decode` in `src/dro_net.rs`
- `decode_image` in `src/dro_paint.rs`
- `delete_since` in `src/browser/history.rs`
- `descendants` in `src/dro_dom.rs`
- `detach` in `src/dro_dom.rs`
- `display_text` in `src/input.rs`
- `document_height` in `src/sparrow_engine/bridge.rs`
- `dom_literal` in `src/dro_dom_js.rs`
- `draw` in `src/dro_font.rs`
- `draw` in `src/dro_media_ui.rs`
- `draw_decoration` in `src/dro_font.rs`
- `draw_text` in `src/dro_font.rs`
- `draw_text_style` in `src/dro_font.rs`
- `dump_dom` in `src/sparrow_engine/bridge.rs`
- `empty` in `src/dro_blocklist.rs`
- `encode_pairs` in `src/forms.rs`
- `ended` in `src/dro_media.rs`
- `eval` in `src/dro_js.rs`
- `eval_js` in `src/engine_host.rs`
- `eval_js` in `src/sparrow_engine.rs`
- `exec` in `src/engine_host.rs`
- `execute` in `src/engine_host.rs`
- `expired` in `src/safety.rs`
- `export` in `src/dro_net.rs`
- `families` in `src/dro_font.rs`
- `fetch` in `src/dro_net.rs`
- `fetch_max` in `src/dro_net.rs`
- `fetch_range` in `src/dro_net.rs`
- `fetch_sub` in `src/dro_net.rs`
- `fetch_sub_page` in `src/dro_net.rs`
- `find_form` in `src/forms.rs`
- `find_in_page` in `src/engine_host.rs`
- `find_in_page` in `src/sparrow_engine/bridge.rs`
- `find_tag` in `src/dro_dom.rs`
- `fire` in `src/dro_dom_js.rs`
- `fire_event` in `src/engine_host.rs`
- `fire_event` in `src/sparrow_engine/bridge.rs`
- `fmt_time` in `src/dro_media_ui.rs`
- `frame` in `src/dro_media.rs`
- `from_bytes` in `src/engine_host.rs`
- `from_bytes` in `src/engine_host.rs`
- `get` in `src/browser/settings.rs`
- `get` in `src/browser/store.rs`
- `get` in `src/browser/store.rs`
- `get` in `src/dro_net.rs`
- `get_or` in `src/browser/settings.rs`
- `get_or` in `src/browser/store.rs`
- `global_blocked` in `src/dro_blocklist.rs`
- `go_back` in `src/browser/tabs.rs`
- `go_forward` in `src/browser/tabs.rs`
- `handle_key` in `src/input.rs`
- `has_audio` in `src/dro_media.rs`
- `has_fonts` in `src/dro_font.rs`
- `hit` in `src/dro_media_ui.rs`
- `hit_test` in `src/engine_host.rs`
- `hit_test` in `src/sparrow_engine.rs`
- `import` in `src/dro_net.rs`
- `index_at` in `src/input.rs`
- `index_at_x` in `src/dro_font.rs`
- `insert_before` in `src/dro_dom.rs`
- `insert_str` in `src/input.rs`
- `install` in `src/dro_dom_js.rs`
- `install_page` in `src/sparrow_engine/bridge.rs`
- `install_panic_hook` in `src/safety.rs`
- `is_alive` in `src/engine_host.rs`

