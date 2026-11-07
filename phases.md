# Sparrow Engine Phases

This roadmap reflects the code currently in the repository, not the original planned-only feature list. Status is based on source inspection and the latest local `cargo test` attempt.

## Current status

- **JavaScript runtime:** Boa 0.17 is implemented in `src/dro_js.rs`. `src/js_engine.rs` selects `boa` (default) or `off`. **V8 has not been implemented or integrated.** Node/V8 checks mentioned in `PATCHES.md` exercised the bootstrap script separately; Sparrow's JavaScript still executes in Boa.
- **Build:** blocked. `cargo test` fails during compilation with five errors:
  - `reqwest::cookie` cannot be imported with the current `reqwest` feature set.
  - `unicode_bidi` is referenced but not declared as a dependency.
  - `unicode_linebreak` is referenced but not declared as a dependency (two diagnostics).
  - `dro_paint::decode_image` is referenced but does not exist.
- **Implication:** no full-crate tests ran; fix the compile blockers before relying on phase-level feature claims.

## Phase 0 — Build and verify

**Status: blocked / first priority**

- [ ] Resolve the five compile errors above, using existing project intent and dependencies.
- [ ] Run `cargo test` to completion and address any further compile or test failures.
- [ ] Run `cargo clippy -- -D warnings`.
- [ ] Keep the build/test status current in this file and `README.md`.

## Phase 1 — Basic page pipeline

**Status: partially implemented**

- [x] HTML parsing, arena DOM, CSS cascade, layout, software painting, and RGBA output are present.
- [x] Charset-aware decoding and richer network request handling are present.
- [x] RAM cookies/cache, redirects, tracker blocking, HTTPS-first behavior, and request-size limits are represented in the network code.
- [ ] Reconcile all pipeline code and dependencies with a successful full build.
- [ ] Improve CSS/layout correctness, including complete selectors, positioning, overflow, and standards behavior.
- [ ] Complete external script loading and other missing web-platform behavior.

## Phase 2 — Real text

**Status: partially implemented**

- [x] System font discovery, shaping, and non-bitmap text rendering paths exist (`dro_font.rs`, `rustybuzz`).
- [x] Bitmap rendering remains available as a fallback.
- [ ] Fix the currently missing Unicode bidi/line-breaking dependencies or remove unsupported references.
- [ ] Finish bidi, line breaking, font fallback, and script coverage; validate Bangla/Indic rendering.
- [ ] Add web font loading and glyph caching.

## Phase 3 — JavaScript and DOM

**Status: partial implementation; runtime is Boa**

- [x] Boa runtime, per-engine context, execution limits, console/alert logging, and inline scripts are implemented.
- [x] `dom_bootstrap.js` provides a partial DOM, selectors, basic mutation, events, timers, storage, location, and media shims.
- [x] A `JsEngine` abstraction exists, but currently supports only `boa` and `off`.
- [ ] Complete and test DOM/lifecycle behavior against the actual Boa runtime.
- [ ] Implement real `fetch`/XHR, modules, external scripts, and standards-compliant asynchronous jobs.
- [ ] V8 is **not** implemented. If V8 is selected as a future goal, add and verify the runtime dependency, adapter, lifecycle/resource management, and build support; Node/V8-only tests do not satisfy this.

## Phase 4 — User interaction

**Status: partial**

- [x] Link hit testing, scrolling, navigation requests from the JS bridge, timers, and media controls have implementation paths.
- [ ] Complete focus, keyboard/input editing, selection, clipboard, forms, and scroll containers.
- [ ] Verify interaction behavior through the shell integration.

## Phase 5 — Performance

**Status: mostly planned**

- [x] RAM caching and parallel sub-resource fetches provide some existing performance support.
- [ ] Add incremental style/layout/paint invalidation, resource prioritization, and measured benchmarks.
- [ ] Evaluate GPU rendering and worker scheduling against actual performance targets.

## Phase 6 — Security and process model

**Status: partial**

- [x] Panic guarding, safety limits, private-network sub-resource checks, and third-party tracker blocking have implementation paths.
- [ ] Add/verify same-origin policy, CORS, CSP, mixed-content behavior, and stronger JS isolation.
- [ ] Run the engine on a dedicated large-stack thread or in a separate process; `catch_unwind` cannot recover from stack overflow or process-level failures.
- [ ] Review cookie and storage lifecycle/security behavior.

## Phase 7 — Extras

**Status: partial**

- [x] Audio/media support and animated GIF playback are present in the source.
- [ ] Complete video frame decoding and broader codec support.
- [ ] Consider SVG, canvas, image formats, diagnostics, and extension hooks.

## Phase exit criteria

Before calling a phase complete:

1. The crate builds on the supported toolchain.
2. Relevant tests pass, including regression coverage for behavior changes.
3. Clippy is clean or any existing exceptions are explicitly recorded.
4. README and this roadmap describe verified behavior rather than intended behavior.
