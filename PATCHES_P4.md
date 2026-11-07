# Phase 4 — what changed (apply on top of your current repo)

## New files (copy as-is)
`src/engine_host.rs` `src/input.rs` `src/forms.rs` `src/dro_cache.rs` `src/browser/store.rs`

## Replaced files (full content provided; based on the copy in your uploaded zip)
| File | Change |
|---|---|
| `Cargo.toml` | `rusqlite` removed (no SQLite, no `bundled` C code) |
| `src/lib.rs` | + `dro_cache`, `engine_host`, `forms`, `input` |
| `src/browser/mod.rs` | + `store` |
| `src/browser/history.rs` | RAM-only (nothing on disk) |
| `src/browser/session.rs` | RAM-only "recently closed tabs" (no restore after restart) |
| `src/browser/bookmark.rs`, `settings.rs` | re-export from `store.rs` (plain text file) |
| `src/ffi.rs` | 3 lines: `SparrowEngine` -> `RemoteEngine` (engine runs on a 256 MB-stack thread) |

If another AI changed `ffi.rs`/`lib.rs`/`mod.rs`/`Cargo.toml` since your zip, re-apply only these edits instead of replacing the file:
- ffi.rs: `use crate::engine_host::RemoteEngine;`, field `engine: RemoteEngine`, constructor `RemoteEngine::new()`
- lib.rs: `pub mod dro_cache; pub mod engine_host; pub mod forms; pub mod input;`
- browser/mod.rs: `pub mod store;`      - Cargo.toml: delete the `rusqlite` line

## API notes
- `Bookmarks::add/remove` and `Settings::set/remove` now take `&mut self` (were `&self`). `History::add/clear` also `&mut self`.
- On disk now: ONLY bookmarks + settings (`Bookmarks::open(path)`, `Settings::open(path)`). Use two different file paths.
- History, closed tabs, HTTP cache: RAM only.

## Not wired yet (needs small patches in files other AIs are editing)
1. Forms: on submit click -> `forms::find_form` + `forms::build_request`; POST needs `dro_net::post(url, body, content_type)`.
2. Cache: in `dro_net` fetch -> `Cache::lookup` first; on `Stale` send `If-None-Match` / `If-Modified-Since`; on 304 call `refresh`; on 200 call `store` with response headers. One `Cache` per engine.
3. Text input: engine keeps `Option<(NodeId, input::TextField)>` for the focused control; key events -> `TextField::handle_key`; on `Outcome::Changed` write `text()` back to the `value` attribute and mark layout dirty; draw caret/selection with `x_of` / `index_at` + `dro_font::measure`.
4. Process isolation (later): `Cmd`/`Reply` already have a binary codec; a `ProcessHost` only spawns a child process and pipes `Cmd::to_bytes()` / `Reply::from_bytes()` frames (u32 length prefix).

## Verified (rustc --test, no cargo)
store 4, history 4, session 2, input 9, forms 6, dro_cache 5, engine_host 8 (stub engine with the same signatures) = 38 tests pass.
NOT verified: ffi.rs against the real engine, and anything needing the whole crate to compile.
