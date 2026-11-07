# Patches to apply on the existing code (all small). Two other AIs are debugging: apply on top of THEIR version.

Everything new is in NEW files. Existing files need only these edits.

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

## Verified here (real compiler / node)
- dro_charset.rs : `rustc --test` -> 4/4 pass
- dro_css_ext.rs : `rustc --test` -> 6/6 pass
- dro_dom_js.rs  : 2/2 pass (with stub JsRuntime)
- dom_bootstrap.js : 24 checks pass in Node/V8 (selectors, events, timers, innerHTML, classList, style, location, storage)
## NOT verified (needs real deps): bridge.rs, ffi.rs, session.rs, settings.rs, and that boa 0.17 accepts dom_bootstrap.js
