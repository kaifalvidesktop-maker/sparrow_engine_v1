# Sparrow Engine

> Rust browser engine embedded inside the **Sparrow** desktop browser shell (Go + WebView shell by Kaif Alvi / Codone).
> The shell calls the engine; the engine returns pixels. **The engine must never crash the shell.**

**বাংলা সারসংক্ষেপ:** এটা Sparrow browser-এর ভেতরের Rust engine। Shell এটাকে URL দেয়, engine HTML/CSS/JS প্রসেস করে RGBA pixel ফেরত দেয়। Engine panic করলেও shell বেঁচে থাকে এবং "Page crashed, reload?" দেখায়। এই README অন্য AI/developer-কে পুরো প্রজেক্ট বোঝানোর জন্য ইংরেজিতে লেখা।

---

## Table of contents

1. [Status at a glance](#1-status-at-a-glance)
2. [What we have covered (implemented)](#2-what-we-have-covered-implemented)
3. [What we will cover (roadmap)](#3-what-we-will-cover-roadmap)
4. [Code structure](#4-code-structure)
5. [System design](#5-system-design)
6. [Packages (25+) for building this engine](#6-packages-25-for-building-this-engine)
7. [Feature reference for AI assistants](#7-feature-reference-for-ai-assistants)
8. [Public API contract](#8-public-api-contract)
9. [Engineering rules (MUST follow)](#9-engineering-rules-must-follow)
10. [Build, run, test](#10-build-run-test)
11. [Known issues and limitations](#11-known-issues-and-limitations)
12. [Performance plan ("Chromium-like")](#12-performance-plan-chromium-like)
13. [Glossary](#13-glossary)

---

## 1. Status at a glance

| Item | State |
|---|---|
| Language / edition | Rust 2021 |
| Code size | ~1,500 lines (target: grow toward 50k lines in phases) |
| Compiled & tested? | **NO.** Code was written without a Rust toolchain that could resolve dependencies. First task for any contributor: run `cargo build`, fix compile errors. |
| Rendering | CPU software rendering (tiny-skia), bitmap font |
| JS | boa_engine interpreter, console/alert only, no DOM API |
| Network | reqwest blocking + rustls, direct connection, no proxy |
| Storage | rusqlite (history, bookmarks) |
| GPU | Not used yet (planned: vello + wgpu) |

Design goal order: **(1) never crash the shell, (2) correct and all pages, (3) small binary, (4) speed.**

---

## 2. What we have covered (implemented)

Legend: ✅ done (written, not yet compiled) · 🟡 partial · ❌ not started

### 2.1 Pipeline

```
URL → fetch → bytes → HTML parse → DOM → CSS parse + cascade → styled DOM
    → layout (taffy) → display list → paint (tiny-skia) → RGBA framebuffer → shell
                                   ↘ JS (boa) runs inline <script> after style
```

### 2.2 Networking (`dro_net.rs`)

- ✅ `fetch(url)` blocking GET via reqwest + rustls, 20 s timeout, `User-Agent: Sparrow/0.1`
- ✅ `.no_proxy()` → direct connection only (no system proxy, no MS/Google service)
- ✅ 20 MB body cap (`Read::take`)
- ✅ Only `http://` and `https://` accepted
- ✅ `resolve(base, href)` → absolute URL via the `url` crate
- ✅ `par_map(urls, f)` → parallel downloads (8 threads at a time, `std::thread::scope`), order preserved. Used for CSS files and images.
- ❌ Cookies, cache, HTTP/2 tuning, gzip/brotli, redirects policy customisation, POST/forms, streaming

### 2.3 HTML parsing (`dro_html.rs`)

- ✅ Full HTML5 spec-compliant parsing through `html5ever` (error recovery, implied tags, tables, foster parenting)
- ✅ Custom `TreeSink` that builds our own arena DOM directly (no intermediate rcdom, no extra crate)
- ✅ Adjacent text merging, attributes, `add_attrs_if_missing`, `reparent_children`, `append_before_sibling`
- 🟡 Comments/PIs stored as `Comment` nodes; doctype ignored; template contents = the template node itself
- ❌ Streaming/incremental parsing, `document.write`, encoding sniffing (assumes UTF-8, lossy)

### 2.2 DOM (`dro_dom.rs`)

- ✅ Arena DOM: `Vec<Node>`, `NodeId = usize`, node `0` is the Document
- ✅ `NodeKind::{Document, Element{tag, attrs}, Text, Comment}`
- ✅ Ops: `create`, `append_child`, `insert_before`, `detach`, `tag`, `attr`, `descendants` (iterative preorder), `text_content`, `find_tag`
- ❌ `get_by_id` index, class list, mutation observers, events, dirty flags

### 2.4 CSS (`dro_css.rs`) — own implementation (the `stylist` crate is Yew-oriented and unsuitable)

- ✅ Built-in UA stylesheet (`UA` constant): block/inline defaults, headings, lists, tables (tr = flex row), inputs, buttons, `hr`, `a`
- ✅ Parser: comments stripped, rule lists, selector lists (`a, b`), declarations, `!important`
- ✅ Selectors: type, `*`, `.class` (multiple), `#id`, compound (`div.a.b#x`), **descendant** (`a b`), **child** (`a > b`)
- ✅ Specificity (id 10000 / class 100 / type 1) + source order + `!important` + inline `style=""`
- ✅ Inheritance: `color`, `font-size`, `font-weight`, `text-decoration`, `text-align`, `line-height`
- ✅ Properties: see table in §7.3
- ❌ Not supported: attribute selectors, pseudo-classes/elements (`:hover`, `::before`), sibling combinators, `@media`, `@font-face`, `@import`, variables (`--x`, `var()`), `calc()`, `vw/vh`, animations, transitions, grid, `position`, `float`, `z-index`, `opacity`, `border-radius`, shadows, gradients, background images

### 2.5 Layout (`dro_layout.rs`) — taffy 0.4

- ✅ Block layout, flexbox (direction, grow, gap, justify, align-items), margins (incl. `auto` left/right), padding, border, `width` (px/%), `max-width`, `height` (px)
- ✅ **Inline layout via word boxes:** text is split into words; each word is a fixed-size taffy leaf; consecutive inline content is wrapped in an anonymous *flex-wrap row* → gives line wrapping, `text-align` (via `justify-content`), links inline, `<br>` (zero-height 100 % wide leaf forces a break)
- ✅ `<img>`: intrinsic size from decoded image, `width`/`height` attributes or CSS, aspect-ratio preserved, `max-width: 100%`
- ✅ `<input>` text (180 px default width, placeholder shown), password (`*`), checkbox/radio (13 px box), submit/button (value as label)
- ✅ `<li>` gets a `*` bullet word
- ✅ Output = flat **display list** (`Vec<Item>`) in paint order with absolute coordinates + document height
- ✅ Canvas background propagated from `<html>` / `<body>` background
- ❌ Real text shaping, bidi, hyphenation, baseline alignment, `position`, `float`, grid, table layout algorithm, margin collapsing, `box-sizing`, overflow/scroll containers, incremental relayout

### 2.6 Paint (`dro_paint.rs`) — tiny-skia 0.11

- ✅ `Pixmap` canvas filled with page background
- ✅ Solid rect fill, 4-side borders (single colour), underline
- ✅ **Built-in 5×7 bitmap font** (ASCII 32–126), scaled by font size, bold = widened columns, unknown glyph → `?`-like cell
- ✅ Images: decode with `image` crate → premultiplied `Pixmap` → drawn with `Pattern` shader, bilinear scaling; broken-image placeholder
- ✅ Viewport culling (items outside viewport are skipped) + vertical scroll offset
- ✅ Output: `Vec<u8>` RGBA, `viewport_w * viewport_h * 4`
- ❌ Real fonts, anti-aliased text, rounded corners, gradients, shadows, SVG, GPU, tiling, damage rects

### 2.7 JavaScript (`dro_js.rs`) — boa_engine 0.17

- ✅ One `Context` per engine instance
- ✅ Safety limits: loop iteration limit 5,000,000; recursion limit 400 (prevents shell freeze on `while(true){}`)
- ✅ `console.log/warn/error/info`, `alert(msg)` → captured into a thread-local log; read with `engine.take_logs()`
- ✅ `document.title`, `document.URL`, `window` set at load
- ✅ Inline `<script>` executed in document order after styles; errors are swallowed (page still renders)
- ✅ `eval_js(code)` returns the result converted to string
- ❌ DOM API (`getElementById`, `querySelector`, `innerHTML`, `createElement`), events, `setTimeout`, `fetch`/XHR, `localStorage`, external `<script src>`, modules, timers/event loop

### 2.8 Engine core (`sparrow_engine.rs`)

- ✅ `SparrowEngine` struct, `new()`, `load_url`, `load_html`, `render`, `eval_js`, `take_logs`, `set_viewport`, `scroll_by`, `hit_test`, `reload`, `recover`, `is_crashed`, `url`, `title`, `viewport`
- ✅ **Crash isolation:** every public method that can fail goes through `guard()` → `catch_unwind(AssertUnwindSafe(..))`. On panic: `crashed = true`, method returns `Err("Page crashed, reload?")`. All later calls return the same error until `recover()`/`reload()`.
- ✅ Lazy layout: computed on first `render`/`scroll_by` after load or viewport change
- ✅ Content-type handling: HTML, `text/*` (shown in `<pre>`), `image/*` (wrapped in `<img>`), others → `Err`

### 2.9 Browser features (`src/browser/`)

- ✅ `omnibox::resolve(input)` → URL or search (DuckDuckGo; not Google/MS). Rules: scheme kept; `about:`; host-like (`has '.'`, no spaces, or `localhost`) → `https://`; else percent-encoded search.
- ✅ `tabs::Tabs` → multiple tabs, each with its **own** `SparrowEngine` (crash in one tab does not affect others), back/forward stacks, `navigate/go_back/go_forward/open/close/switch`
- ✅ `history::History` (SQLite): `add` (upsert + visit count), `search`, `clear`
- ✅ `bookmark::Bookmarks` (SQLite): `add`, `remove`, `contains`, `list`

---

## 3. What we will cover (roadmap)

Each phase should end with: compiles, `cargo clippy` clean, no `unwrap/expect`, tests added, README updated.

### Phase 0 — Make it build (next step, mandatory)
- [ ] `cargo build` and fix compile errors (boa 0.17 and html5ever 0.26 APIs were written from memory)
- [ ] Pin transitive dependencies if the toolchain is old (see §11)
- [ ] Smoke test: render `data` HTML string → PNG dump (add `examples/dump.rs` using the `image` crate)
- [ ] Add `#[cfg(test)]` tests for css parser, selector matching, omnibox, dom

### Phase 1 — Correctness of basic pages
- [ ] `getElementById`-style index in DOM
- [ ] Attribute selectors, `:first-child`, `:last-child`, `:not()`, `:hover` (needs state)
- [ ] `@media (max-width/min-width)`, `vw/vh/vmin/vmax`, `calc()`, CSS variables
- [ ] `position: relative/absolute/fixed`, `z-index`, `float` (basic), `overflow: hidden`, `display: none` via `visibility`
- [ ] `box-sizing`, margin collapsing, `min-/max-height`
- [ ] Charset sniffing (`<meta charset>`, header) with `encoding_rs`
- [ ] gzip/brotli, cookies, redirect handling, HTTP cache (in-memory + SQLite)
- [ ] External `<script src>`

### Phase 2 — Real text
- [ ] Replace bitmap font with real fonts: `fontdb` + `ttf-parser`/`swash` + `parley` for shaping
- [ ] Bidi (`unicode-bidi`), line breaking (`unicode-linebreak`), grapheme clusters (`unicode-segmentation`)
- [ ] **Bangla / Indic script support** (complex shaping through parley/swash/HarfBuzz-like) — required for Bangladeshi users
- [ ] Font fallback chain, `@font-face` with woff2
- [ ] Glyph cache / atlas; anti-aliased text; subpixel positioning

### Phase 3 — JavaScript with DOM
- [ ] Bind DOM to boa: `document.getElementById/querySelector(All)/createElement/appendChild/removeChild`, `innerHTML/textContent`, `setAttribute`, `classList`, `style`
- [ ] Event loop: `setTimeout/setInterval/requestAnimationFrame`, microtasks (boa job queue)
- [ ] Events: click, input, submit, DOMContentLoaded, load
- [ ] `fetch`, `XMLHttpRequest`, `localStorage` (SQLite), `location`, `history`
- [ ] Dirty flags → re-style/re-layout/re-paint after DOM mutation

### Phase 4 — Interaction
- [ ] Hit testing with real boxes (not only links), focus, text input editing, caret, selection, clipboard
- [ ] Form submission (GET/POST), scrolling containers, mouse wheel, smooth scroll
- [ ] Find-in-page, zoom, print-to-PDF

### Phase 5 — Performance ("Chromium-like")
- [ ] Incremental style/layout (invalidate only dirty subtrees)
- [ ] GPU painting with `vello` + `wgpu`, tiled raster, layer compositing
- [ ] Parallel style computation (`rayon`), off-thread parsing and networking
- [ ] Resource scheduler (priority: HTML > CSS > fonts > images), HTTP/2 multiplexing, preconnect
- [ ] Memory budgets, image downscale-on-decode, LRU caches
- [ ] Benchmarks (`criterion`) and perf CI with budgets

### Phase 6 — Security & process model
- [ ] Run engine in a **separate thread with a big stack** or separate process (stack overflow cannot be caught by `catch_unwind`)
- [ ] Same-origin policy, CORS, mixed-content blocking, CSP basics, cookie flags
- [ ] Sandbox JS (no network except via our `fetch`), memory limits, timeouts/watchdog
- [ ] Tracking protection / ad-block list (Brave-style, local only)

### Phase 7 — Extras
- [ ] SVG (`usvg` + `resvg`/vello), canvas 2D (subset), WebP/AVIF images
- [ ] DevTools-lite: DOM tree dump, computed style dump, layout boxes overlay
- [ ] Extension hooks for PuryWeb ("internet purifier")

---

## 4. Code structure

```
sparrow_engine/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs                 # public API: pub fn create_engine() -> SparrowEngine
    ├── sparrow_engine.rs      # SparrowEngine: orchestrates the whole pipeline + crash guard
    ├── dro_net.rs             # fetch, resolve, par_map
    ├── dro_html.rs            # html5ever TreeSink -> Dom
    ├── dro_dom.rs             # arena DOM
    ├── dro_css.rs             # parser, selectors, cascade, computed Style
    ├── dro_layout.rs          # DOM+Style -> taffy tree -> display list
    ├── dro_paint.rs           # display list -> RGBA (tiny-skia), bitmap font, image decode
    ├── dro_js.rs              # boa_engine runtime wrapper
    └── browser/
        ├── mod.rs
        ├── omnibox.rs         # input -> URL/search
        ├── tabs.rs            # tab manager (1 engine per tab)
        ├── history.rs         # SQLite
        └── bookmark.rs        # SQLite
```

### 4.1 Module dependency graph (no cycles)

```
lib.rs ──► sparrow_engine ──► dro_html ──► dro_dom
                │            ├► dro_css  ──► dro_dom
                │            ├► dro_layout ─► dro_dom, dro_css, dro_net(resolve), dro_paint(advance)
                │            ├► dro_paint ──► dro_css(Color), dro_layout(Layout, Paint)
                │            ├► dro_js
                │            └► dro_net
browser/tabs ──► sparrow_engine, browser/omnibox
browser/history, browser/bookmark ──► rusqlite only
```

### 4.2 Key types (cheat sheet)

| Type | File | Meaning |
|---|---|---|
| `Dom`, `Node`, `NodeId`, `NodeKind` | dro_dom | arena tree; `nodes[0]` = Document |
| `Rule`, `Style`, `Color(r,g,b)`, `Display`, `Len`, `Align` | dro_css | parsed rules and computed style (`Vec<Option<Style>>` indexed by `NodeId`) |
| `Layout { items, height, bg }` | dro_layout | result of layout |
| `Item { x, y, w, h, paint }` | dro_layout | one drawable in absolute document coordinates |
| `Paint::{Box, Word, Image}` | dro_layout | what to draw |
| `Response { url, status, content_type, body }` | dro_net | HTTP result (final URL after redirects) |
| `JsRuntime` | dro_js | wraps `boa_engine::Context` |
| `SparrowEngine` | sparrow_engine | the object the shell owns |
| `Tabs`, `Tab` | browser/tabs | tab manager |

### 4.3 Naming conventions

- Engine files are prefixed `dro_` (historical name "Dro"). Browser-level files live in `browser/`.
- Error type everywhere: `Result<T, String>`.
- Colors are `Color(u8,u8,u8)` (alpha not yet supported).
- Coordinates are `f32` CSS pixels (no device-pixel-ratio yet).

---

## 5. System design

### 5.1 Context diagram

```
┌─────────────────────────── Sparrow Shell (Go + WebView, Win32/MinGW) ───────────────────────────┐
│  window, address bar UI, tab strip UI, menus, event loop                                         │
│        │ calls (FFI/binding)                                       ▲ RGBA pixels, strings        │
│        ▼                                                           │                             │
│  ┌──────────────────────── Sparrow Engine (Rust, this repo) ───────┴───────────────┐             │
│  │  Tabs ─► SparrowEngine ─► net/html/css/layout/paint/js                          │             │
│  │  History / Bookmarks (SQLite file)                                              │             │
│  └─────────────────────────────────────────────────────────────────────────────────┘             │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
                         │ direct TCP+TLS (rustls), no proxy, no Google/MS endpoints
                         ▼
                      Internet
```

> The shell integration layer (Rust ↔ Go) is **not written yet**. Options: build engine as a `cdylib` with a C ABI (`extern "C"`) and call it from Go via cgo, or run it as a child process speaking a binary protocol (better crash isolation; recommended for Phase 6).

### 5.2 Data flow for `load_url`

1. `guard()` checks `crashed`; wraps the rest in `catch_unwind`.
2. `dro_net::fetch` → `Response`.
3. Content-type switch → HTML string.
4. `dro_html::parse` → `Dom`.
5. Walk DOM once collecting: `<style>` text, `<link rel=stylesheet>` URLs, `<img src>` URLs, inline `<script>` code, `<title>`.
6. `par_map` downloads CSS + images in parallel; images decoded to premultiplied `Pixmap`.
7. `dro_css::parse(UA)` + author rules in document order → `compute()` → `Vec<Option<Style>>`.
8. New `JsRuntime`; set `document.title/URL`; run scripts (errors ignored).
9. Layout is **lazy**: invalidated (`layout = None`).

`render()`: `ensure_layout()` → `dro_layout::compute(...)` → `dro_paint::paint(...)` → RGBA bytes.

### 5.3 Layout algorithm (important for anyone touching `dro_layout.rs`)

- `block(node)` builds a taffy node for an element. Children are processed by `inline()`.
- `inline()` accumulates a **run** (`Vec<taffy::NodeId>`) of word leaves / inline-block boxes. When a block-level child appears, `flush()` wraps the run into an anonymous `display:flex; flex-wrap:wrap` row and pushes it to the parent's child list, then the block child is added.
- Inline elements (`span`, `a`, `b`…) do **not** create boxes; their text words are emitted into the parent's run carrying the inline element's color/size/bold/link. Consequence: inline padding/background/border are currently ignored.
- Word width = `chars * advance(font_size)`; `advance = max(font_size*0.6, 4)`. Paint uses the same function → **layout and paint must always use `dro_paint::advance`** to stay consistent. When real fonts arrive, replace both with the shaper's measurements.
- After `compute_layout`, `collect()` walks the taffy tree adding parent offsets and emits `Item`s in tree (= paint) order.

### 5.4 Crash-safety model

| Failure | Handled? | How |
|---|---|---|
| Rust panic (index OOB, arithmetic, etc.) | ✅ | `guard()` → `Err("Page crashed, reload?")`; shell shows button; `recover()`/`reload()` resets |
| Error (network, parse, alloc) | ✅ | `Result<T, String>` |
| JS exception | ✅ | returned as `Err("JS error: …")` / swallowed for page scripts |
| JS infinite loop / deep recursion | ✅ | boa runtime limits |
| Stack overflow (deep recursion in Rust) | ❌ | **cannot** be caught; mitigated by depth caps (256/200) in css/layout; real fix = run engine on a thread with a large stack or in a child process |
| Out-of-memory abort | ❌ | caps on body (20 MB), image pixels (16 MP), image count (40); real fix = process isolation |
| Infinite loop in Rust code | ❌ | no watchdog yet (Phase 6) |

Tabs are isolated from each other because every tab owns its engine; a crashed tab is recoverable without touching the others (as long as the panic did not corrupt global state — there is only a `thread_local` log and a `OnceLock` HTTP client).

### 5.4.1 Threading

- Engine objects are **not `Send`-safe by design** (boa `Context` is `!Send`). Keep each `SparrowEngine` on one thread.
- `dro_net::fetch` is **blocking**. The shell must call `load_url` from a worker thread, never the UI thread. (Phase 5: async fetch + message passing.)
- `par_map` uses scoped threads internally for CSS/image downloads.

### 5.5 Target architecture (Phase 5–6)

```
UI thread (shell)            Engine thread/process                  Worker pool (rayon)
  input events ──────────►   task queue ──► Parser ──► Style ─────► parallel style / decode images
  ◄── frames (RGBA/GPU) ───  Layout ──► Display list ──► Raster/Compositor (vello+wgpu)
                             JS thread (boa) ◄──► DOM bindings ──► dirty flags ──► re-style/layout/paint
                             Network service (async reqwest, cache, cookies) 
```

### 5.6 Storage

SQLite file(s) chosen by the shell (`History::open(path)`, `Bookmarks::open(path)`).

```sql
history(url TEXT PRIMARY KEY, title TEXT, visits INTEGER DEFAULT 1, last INTEGER)
bookmarks(id INTEGER PRIMARY KEY, url TEXT UNIQUE, title TEXT)
```

### 5.7 Design principles

1. **No vendor lock-in / no vendored code:** dependencies come from crates.io only; no copied/forked third-party source in the repo. (Note: `rusqlite`'s `bundled` feature compiles SQLite's C source that ships inside the crate.) No Google/Microsoft services; default search engine is DuckDuckGo.
2. **Small binary:** `default-features = false` wherever possible; `opt-level="z"`, `lto`, `strip`, `codegen-units=1`.
3. **Own the core:** CSS and layout glue are ours; heavy lifting delegated to focused crates.
4. **Fail soft:** a broken stylesheet/script/image never prevents the page from rendering.

---

## 6. Packages (25+) for building this engine

Status: **USED** = in `Cargo.toml` now · **PLAN** = planned, within the original 15-library list · **PROPOSE** = outside the original 15, needs owner approval before adding (the owner restricted the engine to a fixed library list).

| # | Crate | Purpose in Sparrow | Status |
|---|---|---|---|
| 1 | `html5ever` 0.26 | HTML5 tokenizer + tree builder | USED |
| 2 | `markup5ever` | names/atoms used by html5ever (transitive) | USED (transitive) |
| 3 | `taffy` 0.4 | block + flexbox layout (grid later) | USED |
| 4 | `boa_engine` 0.17 | JavaScript interpreter | USED |
| 5 | `tiny-skia` 0.11 | CPU 2D rasteriser (current paint backend) | USED |
| 6 | `reqwest` 0.11 (`blocking`,`rustls-tls`) | HTTP client | USED |
| 7 | `rustls` | TLS, no OpenSSL / no OS dependency | USED (transitive) |
| 8 | `image` 0.24 (`png`,`jpeg`,`gif`) | image decoding | USED |
| 9 | `rusqlite` 0.30 (`bundled`) | history, bookmarks, later cookies/cache/localStorage | USED |
| 10 | `url` 2.4 | URL parsing/joining | USED |
| 11 | `serde` 1.0 | (de)serialisation of settings, IPC messages | USED (declared, light use) |
| 12 | `parley` 0.1 | text layout/shaping API | PLAN |
| 13 | `vello` 0.1 | GPU 2D renderer (compute-based) | PLAN |
| 14 | `wgpu` 0.19 | GPU abstraction (must match vello's wgpu version) | PLAN |
| 15 | `winit` 0.28 | windowing — only for a standalone test harness (the shell owns the real window) | PLAN |
| 16 | `usvg` 0.36 | SVG parsing → paths (renders via vello/tiny-skia) | PLAN |
| 17 | `stylist` 0.12 | **Not suitable** (Yew-focused). Replaced by own `dro_css`. | REJECTED |
| 18 | `cssparser` | robust CSS tokenizer (Servo) — replace hand-rolled parser | PROPOSE |
| 19 | `selectors` | spec-correct selector matching incl. pseudo-classes | PROPOSE |
| 20 | `fontdb` | system/web font discovery & matching | PROPOSE |
| 21 | `swash` / `ttf-parser` | glyph outlines, shaping fallback, metrics | PROPOSE |
| 22 | `unicode-bidi` | bidirectional text (Arabic/Hebrew mixed with LTR) | PROPOSE |
| 23 | `unicode-linebreak` | UAX #14 line breaking | PROPOSE |
| 24 | `unicode-segmentation` | grapheme clusters (Bangla conjuncts, emoji) | PROPOSE |
| 25 | `encoding_rs` | charset decoding (legacy encodings) | PROPOSE |
| 26 | `flate2` / `brotli` | gzip/brotli response decoding | PROPOSE |
| 27 | `rayon` | parallel style computation / image decode | PROPOSE |
| 28 | `crossbeam-channel` | engine ⇄ worker/UI messaging | PROPOSE |
| 29 | `lru` | image / font / HTTP caches | PROPOSE |
| 30 | `hashbrown` / `ahash` | faster hash maps for style/layout tables | PROPOSE |
| 31 | `smallvec` | avoid heap allocs for small child lists | PROPOSE |
| 32 | `bumpalo` | arena allocation for per-frame temporary data | PROPOSE |
| 33 | `cookie_store` | cookie jar with RFC 6265 rules | PROPOSE |
| 34 | `tracing` | structured logs/profiling spans | PROPOSE |
| 35 | `criterion` | benchmarks (dev-dependency) | PROPOSE |
| 36 | `proptest` | fuzz-like property tests for parsers (dev-dependency) | PROPOSE |
| 37 | `lightningcss` | alternative full CSS parser/minifier | PROPOSE (alternative to 18) |

> If the 15-library rule must stay strict, Phases 0–1 and the `parley`/`vello`/`wgpu`/`usvg` parts of Phases 2/5/7 fit inside it. The rest require an explicit decision.

---

## 7. Feature reference for AI assistants

### 7.1 HTML elements and default behaviour

| Element(s) | Behaviour |
|---|---|
| `html body div p h1–h6 ul ol li section article header footer nav main aside form table thead tbody tfoot pre blockquote figure fieldset dl dt dd address center details summary` | `display:block` |
| `tr` | flex row; `td`, `th` = block, `flex-grow:1`, padding 2px (cheap table support, columns do not align across rows) |
| `a` | inline, blue, underlined; `href` makes words clickable (`hit_test`) |
| `b strong th` | bold |
| `img` | inline-block, loaded from `src` (png/jpeg/gif) |
| `input` (text/password/checkbox/radio/submit/button/reset/hidden) | see §2.5; not interactive yet |
| `button` | inline-block with border and background |
| `hr` | 1px grey line |
| `br` | forced line break |
| `head script style title meta link template noscript option datalist svg canvas audio video iframe` | `display:none` |

### 7.2 Selectors supported

`tag`, `*`, `.a`, `.a.b`, `#id`, `tag.a#id`, `A B` (descendant), `A > B` (child), comma lists.
Anything containing other characters (`:`, `[`, `+`, `~`, `@`) is **dropped silently** (the rule is ignored).

### 7.3 CSS properties supported

| Property | Values / notes |
|---|---|
| `display` | `none`, `block`, `list-item`, `table`, `flow-root` → block; `flex`, `grid`, `table-row` → flex (grid is faked as flex); `inline-block`, `inline-flex`, … → inline-block; else inline |
| `color` | `#rgb`, `#rrggbb`, `#rrggbbaa` (alpha ignored), `rgb()/rgba()`, ~20 named colors |
| `background`, `background-color` | color only (gradients/images ignored); `none/transparent` clears |
| `font-size` | px, em, rem, pt, %, keywords (small…xx-large, smaller, larger); min 6px |
| `font-weight` | `bold`, `bolder`, ≥600 |
| `text-decoration` | `underline` only |
| `text-align` | left, center, right (end) |
| `line-height` | number, px, `normal` (default 1.3) |
| `margin`, `margin-*` | 1–4 values; px/em/rem/pt; `auto` only for left/right |
| `padding`, `padding-*` | 1–4 values |
| `width`, `max-width` | px, em, rem, %, auto |
| `height` | px |
| `border` | width + color (style keyword ignored), `none`; `border-width`, `border-color` (single value for all sides) |
| `flex-direction` | row / column |
| `flex`, `flex-grow` | first number only |
| `gap`, `column-gap`, `row-gap` | one value |
| `justify-content` | flex-start, center, flex-end, space-between, space-around/evenly |
| `align-items` | stretch, center, flex-start, flex-end |

Everything else is parsed and ignored.

### 7.4 Public data returned to the shell

- `render()` → `Vec<u8>` RGBA8, **premultiplied = straight** because the canvas is fully opaque; row-major, top-left origin, length `w*h*4`.
- `take_logs()` → `Vec<String>` (console output, `alert: <msg>` lines).
- `hit_test(x,y)` → absolute URL string if a link (text or image) is under the point (viewport coordinates; scroll is applied inside).
- `title()`, `url()` → strings; `url()` is the **final** URL after redirects.

### 7.5 How to add a new CSS property (worked example: `opacity`)

1. Add field `opacity: f32` to `Style` and to `Style::root()` (non-inherited → default 1.0).
2. Parse it in `apply()` in `dro_css.rs` (`"opacity" => …`).
3. Carry it into `Paint::Box/Word/Image` in `dro_layout.rs` (`box_paint`, `words`, `image`).
4. Apply it in `dro_paint.rs` (e.g. multiply alpha before `fill_rect`).
5. Add a test in `dro_css.rs` and update §7.3 here.

### 7.6 How to add a new HTML element behaviour

1. Default display → add to `UA` string in `dro_css.rs`.
2. Special content (like `input`) → add a branch in `block()` in `dro_layout.rs`.
3. New drawable → add a `Paint` variant, emit it in layout, draw it in `dro_paint::paint`.

### 7.7 How to expose a new JS function

- Plain function: write `fn f(this:&JsValue, args:&[JsValue], ctx:&mut Context<'_>) -> JsResult<JsValue>` and register with `ctx.register_global_builtin_callable(name, argc, NativeFunction::from_fn_ptr(f))` in `JsRuntime::new`.
- Anything that needs engine state (DOM!) cannot use a plain fn pointer; use `NativeFunction::from_copy_closure_with_captures` with a traced capture holding an `Rc<RefCell<Dom>>`-like handle (Phase 3 design task: the DOM must become shareable between engine and JS).

---

## 8. Public API contract

```rust
// lib.rs
pub fn create_engine() -> SparrowEngine;

impl SparrowEngine {
    pub fn new() -> Self;
    pub fn load_url(&mut self, url: &str) -> Result<(), String>;
    pub fn load_html(&mut self, html: &str, base_url: &str) -> Result<(), String>;
    pub fn render(&mut self) -> Result<Vec<u8>, String>;          // RGBA, viewport size
    pub fn eval_js(&mut self, code: &str) -> Result<String, String>;
    pub fn take_logs(&mut self) -> Vec<String>;
    pub fn set_viewport(&mut self, w: u32, h: u32);
    pub fn viewport(&self) -> (u32, u32);
    pub fn scroll_by(&mut self, dy: f32) -> Result<f32, String>;  // returns new scroll_y
    pub fn hit_test(&self, x: f32, y: f32) -> Option<String>;
    pub fn reload(&mut self) -> Result<(), String>;
    pub fn recover(&mut self);
    pub fn is_crashed(&self) -> bool;
    pub fn url(&self) -> &str;
    pub fn title(&self) -> &str;
}

// browser/
omnibox::resolve(&str) -> String
Tabs::{new, open, close, switch, list, active, navigate, go_back, go_forward}
History::{open, add, search, clear}
Bookmarks::{open, add, remove, contains, list}
```

Shell usage:

```rust
let mut engine = sparrow_engine::create_engine();
engine.set_viewport(1280, 720);
match engine.load_url("https://example.com").and_then(|_| engine.render()) {
    Ok(rgba) => blit(rgba),
    Err(msg) if msg == "Page crashed, reload?" => { show_crash_page(); /* on click: */ engine.recover(); }
    Err(msg) => show_error(msg),
}
let _ = engine.eval_js("alert(1)");
for line in engine.take_logs() { /* "alert: 1" */ }
```

**API stability rule:** do not rename or change these signatures without updating this README and the shell. Add new methods instead.

---

## 9. Engineering rules (MUST follow)

1. **No `unwrap()`, no `expect()`.** Use `?`, `ok_or`, `map_err`, `unwrap_or`, `let … else`. Return `Result<T, String>`.
2. **All engine entry points go through `SparrowEngine::guard`** (`catch_unwind`). New public mutating methods must use it.
3. **Cargo profile must keep `panic = "unwind"`.** With `panic = "abort"`, `catch_unwind` does nothing and Rule 2 is void.
4. **Direct connection only.** `.no_proxy()`, rustls, no Google/Microsoft endpoints, no telemetry.
5. **Small binary.** `default-features = false`; every new dependency must justify its size.
6. **Keep API consistent across files** (names in §4.2/§8). Prefix engine modules with `dro_`.
7. **Never trust input:** cap sizes, depth, counts; no unbounded recursion on DOM depth (use depth caps or iterative walks).
8. **Failure of a sub-resource (CSS, image, script) must not fail the page.**
9. **Indexing (`vec[i]`) can panic** — acceptable inside `guard`, but prefer `.get()` in new code.
10. Update this README when behaviour or API changes.

---

## 10. Build, run, test

```bash
# toolchain: stable Rust (recent). If cargo is old, see §11.
cargo build
cargo build --release          # size-optimised (opt-level z, LTO, strip)
cargo clippy -- -D warnings
cargo test
```

Suggested harness (not written yet): `examples/dump.rs`

```rust
fn main() -> Result<(), String> {
    let mut e = sparrow_engine::create_engine();
    e.set_viewport(1024, 768);
    e.load_html("<h1>Hello</h1><p>World <a href='/x'>link</a></p>", "https://example.test/")?;
    let rgba = e.render()?;
    // write PNG with the `image` crate: image::save_buffer("out.png", &rgba, 1024, 768, image::ColorType::Rgba8)
    let _ = rgba;
    Ok(())
}
```

Test ideas: CSS color parsing, selector matching (descendant/child/specificity), cascade order with `!important`, omnibox rules, DOM operations, layout of a known page against expected word positions, crash guard (inject a deliberate panic behind a `#[cfg(test)]` hook and assert `Err("Page crashed, reload?")`).

---

## 11. Known issues and limitations

1. **Never compiled.** Expect API mismatches, especially: `boa_engine` 0.17 (`register_global_builtin_callable`, `runtime_limits_mut`, `Context<'static>`), `html5ever` 0.26 `TreeSink` signatures, `taffy` 0.4 field names (`Dimension::Length`, `compute_layout`), `tiny-skia` `Pattern::new` args.
2. **Old Cargo (1.75) cannot resolve some latest transitive crates** that require `edition2024` (seen: `icu_properties_data`, `encoding_rs 0.8.44`). Use a current stable Rust, or pin with `cargo update -p <crate> --precise <ver>` (e.g. `url 2.5.0`).
3. **`panic = "abort"` from the original spec was changed to `"unwind"`** because abort makes crash isolation impossible. Costs a little binary size.
4. **`stylist`, `parley`, `vello`, `wgpu`, `winit`, `usvg` are not wired yet.** `stylist` will not be used at all.
5. **Text:** only ASCII is drawn (bitmap font). Bangla/other scripts render as placeholder glyphs. Highest-priority functional gap.
6. **Layout accuracy:** inline backgrounds/padding ignored; table columns don't align; `position`/`float`/grid missing; no margin collapsing.
7. **Blocking network** on the calling thread; call from a worker thread.
8. **Stack overflow / OOM / infinite Rust loops** cannot be caught by `catch_unwind`.
9. **JS has no DOM**; most real websites rely on JS and will look empty or wrong.
10. **Single global `thread_local` log** in `dro_js`: two engines on one thread share the log. Move into the runtime struct when DOM binding is added.
11. `serde` is declared but barely used; remove or use for IPC/settings.
12. HTTPS-only sites with unusual TLS configs may fail (rustls strictness); no certificate UI.

---

## 12. Performance plan ("Chromium-like")

Honest baseline: a ~1.5k-line CPU renderer will not match Chromium. Chromium's speed comes from architecture, not one trick. The plan below lists the techniques and where they land.

| Technique | Chromium analogue | Sparrow plan |
|---|---|---|
| Multi-process + threads | browser/renderer/GPU/network processes | engine thread/process per tab, network service, worker pool (Phase 5–6) |
| Incremental pipeline | style/layout/paint invalidation | dirty flags on DOM nodes; re-run only affected subtrees (Phase 3/5) |
| GPU raster | Skia/GPU, compositor | `vello` + `wgpu`, layers, tiling (Phase 5) |
| Text perf | glyph cache, HarfBuzz | `parley`/`swash` + glyph atlas (Phase 2) |
| Network perf | HTTP/2, preconnect, cache | reqwest H2, connection reuse (already pooled), HTTP cache, resource priorities |
| Parsing perf | streaming parser, preload scanner | stream html5ever, early CSS/image fetch (Phase 5) |
| Memory | discardable images, caches | LRU caches, downscale on decode, budgets |
| JS perf | V8 JIT | boa is an interpreter → expect slow JS; long-term option: evaluate other engines (needs library-list approval) |

Measure first: add `criterion` benches for parse, style, layout, paint on fixed HTML fixtures; track time and peak memory per commit.

---

## 13. Glossary

- **Shell** – the outer Sparrow browser app (window, UI chrome). Calls the engine.
- **Engine** – this crate: turns URLs/HTML into pixels and runs JS.
- **DOM** – tree of nodes representing the document.
- **Cascade** – process choosing final CSS values by specificity/importance/order.
- **Computed style** – final per-element style (`Style`).
- **Display list** – flat ordered list of drawing commands (`Vec<Item>`).
- **Run / word box** – our inline layout trick: each word is a flex item in a wrapping row.
- **Guard** – `catch_unwind` wrapper that keeps panics from reaching the shell.
- **UA stylesheet** – built-in default CSS of the browser.
- **Premultiplied alpha** – colour channels already multiplied by alpha (tiny-skia requirement).
- **Viewport** – visible area (`w × h`); document may be taller → `scroll_y`.
- **Dro** – internal prefix of engine modules.

---

*Owner: Kaif Alvi · Org: Codone · Project: Sparrow Browser. Keep this file current — it is the single source of truth for humans and AI assistants working on the engine.*

---

## Addendum — Phase 3 (fonts, media, tracker blocking)

| Module | Purpose | Verified |
|---|---|---|
| `dro_font.rs` | system fonts (`fontdb`) + OpenType shaping (`rustybuzz`) + AA outline rendering (tiny-skia); Bangla/Indic conjuncts, ligatures, per-char font fallback | compiled, 2 tests, rendered PNG checked visually |
| `dro_media.rs` | pure-Rust audio player: `symphonia` decode, linear resampler, `cpal` output, audio-clock master, seek/volume/mute; animated GIF video; MP4 probe | compiled, 4 tests |
| `dro_media_ui.rs` | Chromium-style control bar (play/pause, time, progress, mute, volume) draw + hit-test | compiled, 3 tests |
| `sparrow_engine/media_bridge.rs` | `<video>/<audio>` slots, threaded download, JS commands, frame upload, overlay, click/key | type-checked against a stub engine |
| `dro_blocklist.rs` | third-party tracker/ad blocking for sub-resources, hosts/ABP-style lists | compiled, 3 tests |
| `dom_bootstrap.js` | + `HTMLMediaElement` API (`play/pause/currentTime/volume/muted/duration`, events) | 33 Node checks |

Not available in pure Rust today: H.264 / VP9 video decoding (only AV1 via `rav1d`, needs rustc >= 1.79). MP4/WebM audio plays; their pictures do not yet.
