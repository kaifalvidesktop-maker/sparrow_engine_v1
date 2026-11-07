# JS/DOM bridge implementation notes

The changes described here are already present in the repository; this file is historical implementation guidance, not a patch set to apply again. Check the current source and `Cargo.toml` before relying on an older snippet. The Node/V8 checks below tested the bootstrap script separately and do **not** mean Sparrow uses V8. The engine's runtime is Boa; see `README.md` and `phases.md`.

The numbered sections below preserve the original file-by-file integration notes.

## 1. Cargo.toml
```toml
[lib]
crate-type = ["rlib", "cdylib"]     # cdylib = C ABI for the shell (ffi.rs)

# reqwest: add "cookies"
reqwest = { version = "0.11", default-features = false, features = ["blocking", "rustls-tls", "cookies"] }
```

## 2. src/dro_net.rs  (cookie jar, in the client builder)
```rust
reqwest::blocking::Client::builder()
    .use_rustls_tls()
    .cookie_store(true)      // <- add
    .no_proxy()
```

## 3. src/lib.rs  (add)
```rust
pub mod dro_charset;
pub mod dro_css_ext;
pub mod dro_dom_js;
pub mod ffi;
```

## 4. src/browser/mod.rs  (add)
```rust
pub mod session;
pub mod settings;
```

## 5. src/sparrow_engine.rs
a) after the `use` lines: `mod bridge;`   (file: src/sparrow_engine/bridge.rs  -> child module, may read private fields)
b) struct `SparrowEngine`: add fields
```rust
    link_css: Vec<String>,
    pending_nav: Option<String>,
```
   and in `new()`: `link_css: vec![], pending_nav: None,`
c) `load_url`: charset-aware decode + new installer
```rust
let text = crate::dro_charset::decode(&r.body, &r.content_type);   // was String::from_utf8_lossy(..)
...
e.install_page(&html, &r.url)                                       // was e.install(..)
...
return e.install_page("", "about:blank");                           // was e.install(..)
```
d) `load_html`: `e.install_page(&h, &b)`
e) delete the old private `fn install(...)` (replaced by `bridge::install_page`)
f) `set_viewport` body becomes: `let _ = self.resize(w, h);`   (re-styles for media queries / vw / vh)

## 6. Shell frame loop
```
engine.run_timers(16); if let Some(u) = engine.take_navigation() { engine.load_url(&u)?; }
let px = engine.render()?;
```

## Component checks recorded when these changes were prepared
- dro_charset.rs : `rustc --test` -> 4/4 pass
- dro_css_ext.rs : `rustc --test` -> 6/6 pass
- dro_dom_js.rs  : 2/2 pass (with stub JsRuntime)
- dom_bootstrap.js : 24 checks pass in Node/V8 (selectors, events, timers, innerHTML, classList, style, location, storage)
The component checks above do not establish that the current full crate compiles or that the bootstrap script works under the in-engine Boa runtime. The latest `cargo test` fails during compilation; see `phases.md`.
