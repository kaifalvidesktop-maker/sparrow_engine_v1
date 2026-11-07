# Changes in this build (all written by reading code — NOT compiled here; run `cargo build` first)

Build breakers fixed
- Cargo.toml: reqwest now has `cookies, gzip, brotli, deflate, http2` (dro_net uses reqwest::cookie).
- dro_paint: added `decode_image()` (PNG/JPEG/GIF, size-checked, premultiplied).
- dro_layout: added `Paint::Image { src, link }`; `<img>`, `<video>`, `<audio controls>` boxes (`media:<n>` keys).
  `compute()` signature is now `compute(dom, styles, width, base_url, image_sizes)`.
- dro_paint: `paint(layout, w, h, scroll, &images)` draws images/media frames (bilinear scaling).

Features wired in
- dro_font is used for measuring (layout) and drawing (paint) when system fonts exist -> Bangla shaping.
  Bitmap font stays as fallback.

Crash / safety
- Viewport validated with `safety::limits::viewport_ok` (max 16M pixels, 16384 per side) in resize/set_viewport.
- `MAX_DOM_NODES` and `MAX_SCRIPT_BYTES` are now enforced.
- Console log buffer bounded (1000 lines x 4000 chars).
- Redirects into the private network are blocked (when block_private_subresources is on).

Correctness
- install_page parses first; a failed load no longer leaves old DOM under the new URL; status reset.
- Only classic JavaScript <script> types run (no JSON-LD / templates / modules).
- Percent-encoded fragments (#%E0%A6%AC) resolve to Bangla ids.
- Linked images are clickable (hit_test).

NOT fixed (needs design work)
- A JS infinite loop wrapped in try/catch can still hang the calling thread (Boa has no interrupt).
  Real fix: run each engine on its own big-stack thread and use recv_timeout from the FFI layer.
- `set_page()` in dro_net is process-global (wrong with several tabs on several threads).
- SameSite not enforced when sending cookies; HTTP cache ignores `Vary`.
- DNS-rebinding style private-network access (public name -> 127.0.0.1) is not blocked.
- Layout has no `white-space: pre`, so text/plain and JSON pages lose line breaks.
