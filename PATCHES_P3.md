# Phase 3 patches (fonts + media + tracker blocking). Apply on top of Phase 1-2 code.

New files (copy as-is): src/dro_font.rs, dro_media.rs, dro_media_ui.rs, dro_blocklist.rs,
src/sparrow_engine/media_bridge.rs, and the UPDATED src/dom_bootstrap.js (adds HTMLMediaElement API).

## Cargo.toml
```toml
symphonia  = { version = "0.5", default-features = false, features = ["mp3","aac","flac","vorbis","ogg","wav","isomp4","mkv","pcm"] }
cpal       = "0.15"        # audio output (Windows: WASAPI, no extra libs; Linux needs libasound2-dev)
mp4        = "0.14"        # MP4 track info (video size/fps/codec)
rustybuzz  = "0.13"        # pure-Rust HarfBuzz (Bangla/Indic shaping)
fontdb     = { version = "0.18", default-features = false, features = ["fs", "memmap"] }
image      = { version = "0.24", default-features = false, features = ["png", "jpeg", "gif"] }   # gif already needed
```
tiny-skia: keep `features = ["std"]`; add `"png-format"` only if you want pm.save_png in tests.
If cargo is old (<1.80): `cargo update -p url --precise 2.5.0` and `cargo update -p encoding_rs --precise 0.8.34`.

## src/lib.rs
```rust
pub mod dro_font; pub mod dro_media; pub mod dro_media_ui; pub mod dro_blocklist;
```

## src/dro_net.rs
```rust
pub fn fetch(url: &str) -> Result<Response, String> { fetch_max(url, 20 * 1024 * 1024) }
pub fn fetch_max(url: &str, max: u64) -> Result<Response, String> { /* old fetch body; replace `.take(20 * 1024 * 1024)` with `.take(max)` */ }
// par_map: skip blocked sub-resources (page URL passed as `page`)
pub fn par_map<T: Send>(urls: &[String], page: &str, f: impl Fn(Response) -> Option<T> + Sync) -> Vec<Option<T>>
//   inside the spawn closure:  if crate::dro_blocklist::global_blocked(u, page) { None } else { fetch(u).ok().and_then(|r| f(r)) }
```
Update the 3 `par_map(&urls, ...)` calls in bridge.rs to pass `base` as 2nd argument.

## src/dro_paint.rs  (real text, bitmap font stays as fallback)
At the start of `fn text(...)`:
```rust
if crate::dro_font::draw_text(pm, t, x, y, h, size, bold, (c.0, c.1, c.2)) { return; }
```
## src/dro_layout.rs
a) word width (in `words()`): replace `w.chars().count() as f32 * adv` with
```rust
if crate::dro_font::available() { crate::dro_font::measure(w, size, bold) } else { w.chars().count() as f32 * adv }
```
   and the trailing-space margin `adv` with `if crate::dro_font::available() { crate::dro_font::measure(" ", size, bold) } else { adv }`.
b) media boxes. In `struct B` add `media_n: usize` (init 0). In `block()` before the `img` check:
```rust
if tag == "video" || tag == "audio" { return self.media_box(n, s, tag == "audio").map(Some); }
```
and a new method:
```rust
fn media_box(&mut self, n: NodeId, s: &Css, audio: bool) -> R<N> {
    let idx = self.media_n; self.media_n += 1;
    let key = format!("media:{idx}");
    let dom = self.dom;
    let controls = dom.attr(n, "controls").is_some();
    let (iw, ih) = self.imgs.get(&key).map(|&(a, b)| (a as f32, b as f32)).unwrap_or((0.0, 0.0));
    let num = |k: &str| dom.attr(n, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
    let (w, h) = if audio { if controls { (num("width").unwrap_or(320.0), 36.0) } else { (0.0, 0.0) } } else {
        match (num("width"), num("height")) { (Some(w), Some(h)) => (w, h), (Some(w), None) => (w, if iw > 0.0 { w * ih / iw } else { w * 0.5625 }),
          (None, Some(h)) => (if ih > 0.0 { h * iw / ih } else { h * 1.7778 }, h), _ => if iw > 0.0 { (iw, ih) } else { (300.0, 150.0) } } };
    let style = taffy::Style { size: Size { width: Dimension::Length(w), height: Dimension::Length(h) }, max_size: Size { width: Dimension::Percent(1.0), height: Dimension::Auto }, flex_shrink: 0.0, ..Default::default() };
    let node = self.tree.new_leaf(style).map_err(er)?;
    if w > 0.0 { self.paints.insert(node, Paint::Image { src: key, link: None }); }
    Ok(node)
}
```
c) UA stylesheet: remove `audio,video` from the `display:none` list and add `video,audio{display:inline-block}`.

## src/sparrow_engine.rs
- `mod media_bridge;`   fields: `media: Vec<media_bridge::Slot>` (init `vec![]`)
- `bridge.rs::install_page`: after `self.restyle();` add `self.setup_media(base);`
- `render()`: wrap the result: `let px = dro_paint::paint(...)?; Ok(e.overlay_media(px))`
- `recover()/new()`: media is dropped automatically (threads stop in `Drop`).

## Shell frame loop
```
engine.run_timers(16);
if engine.media_tick()? || needs_redraw { frame = engine.render()?; }
on mouse click: if !engine.media_click(x, y) { if let Some(u) = engine.hit_test(x, y) { load } }
on key:         engine.media_key(key);
```
## ffi.rs additions (same pattern as other functions)
`sparrow_media_tick(h)->int`, `sparrow_media_click(h,x,y)->int`, `sparrow_media_key(h,key)->int`.

## Known limits
- Audio: mp3, aac(m4a/mp4), flac, ogg-vorbis, wav, mkv-audio. NOT: opus, wma.
- Video pictures: animated GIF only. MP4/WebM video tracks are probed (size/fps/codec) and their AUDIO plays,
  but frames need a decoder. Pure-Rust options: AV1 via `rav1d` (needs rustc >= 1.79). No pure-Rust H.264/VP9 exists.
- Whole file is downloaded before playback (cap 256 MB); no HTTP range requests / streaming / HLS / DASH / MSE yet.
- No fullscreen, no subtitles, no playbackRate.
