//! Child module of `sparrow_engine`: connects <video>/<audio> elements to `dro_media::Media`.
//! Fetch happens on a worker thread (bytes only); `Media` (cpal stream is !Send) is built on the engine thread.
//! Video frames are injected into `self.images` under the key "media:<index>", so the existing
//! image painter draws them; the control bar is overlaid in `overlay_media`.
use super::*;
use crate::dro_layout as lay;
use crate::dro_media::Media;
use crate::dro_media_ui as ui;
use std::sync::mpsc::{channel, Receiver};

pub(super) struct Slot {
    rx: Option<Receiver<Result<(Vec<u8>, String), String>>>,
    media: Option<Media>,
    controls: bool,
    autoplay: bool,
    looping: bool,
    last_time_event: f64,
    was_paused: bool,
    ended_sent: bool,
    frame_key: usize, // address of last frame uploaded (change detection)
}

fn spawn_fetch(url: String, page: String) -> Receiver<Result<(Vec<u8>, String), String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = if crate::dro_blocklist::global_blocked(&url, &page) {
            Err("blocked by tracker protection".to_string())
        } else {
            dro_net::fetch_max(&url, 256 * 1024 * 1024).map(|r| { let h = hint_of(&url, &r.content_type); (r.body, h) })
        };
        let _ = tx.send(r);
    });
    rx
}

fn hint_of(url: &str, ct: &str) -> String {
    let ext = url.split(['?', '#']).next().unwrap_or("").rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext.len() <= 5 && !ext.is_empty() && !ext.contains('/') { return ext; }
    ct.split('/').nth(1).unwrap_or("").split(';').next().unwrap_or("").to_string()
}

fn premultiplied(f: &crate::dro_media::Frame) -> Option<Pixmap> {
    let mut pm = Pixmap::new(f.w, f.h)?;
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(f.rgba.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = (s[0] as u32 * a / 255) as u8;
        d[1] = (s[1] as u32 * a / 255) as u8;
        d[2] = (s[2] as u32 * a / 255) as u8;
        d[3] = s[3];
    }
    Some(pm)
}

impl SparrowEngine {
    /// Create one slot per <video>/<audio> (document order == JS `mediaList()` order) and start fetching.
    pub(super) fn setup_media(&mut self, base: &str) {
        self.media.clear();
        for n in self.dom.descendants(0) {
            if !matches!(self.dom.tag(n), Some("video") | Some("audio")) { continue; }
            // src attribute or first <source src>
            let mut src = self.dom.attr(n, "src").map(|s| s.to_string());
            if src.is_none() {
                src = self.dom.nodes[n].children.iter().find(|&&c| self.dom.tag(c) == Some("source")).and_then(|&c| self.dom.attr(c, "src")).map(|s| s.to_string());
            }
            let rx = src.and_then(|s| dro_net::resolve(base, &s)).filter(|u| u.starts_with("http")).map(|u| spawn_fetch(u, base.to_string()));
            self.media.push(Slot {
                rx, media: None,
                controls: self.dom.attr(n, "controls").is_some(),
                autoplay: self.dom.attr(n, "autoplay").is_some(),
                looping: self.dom.attr(n, "loop").is_some(),
                last_time_event: -1.0, was_paused: true, ended_sent: false, frame_key: 0,
            });
        }
    }

    fn js_media_event(&mut self, i: usize, ev: &str) {
        if let Some(js) = self.js.as_mut() { let _ = js.eval(&format!("__mediaEvent({i},'{ev}')")); }
    }

    /// Drive media: finish downloads, run JS commands, upload frames, fire events.
    /// Call every frame (~16 ms). Returns true when the picture changed and `render()` should run.
    pub fn media_tick(&mut self) -> Result<bool, String> {
        self.guard(|e| {
            if e.media.is_empty() { return Ok(false); }
            let mut redraw = false;
            let page = e.url.clone();

            // 1) JS commands
            let cmds = e.js.as_mut().and_then(|j| j.eval("__mediaCmds()").ok()).unwrap_or_default();
            for line in cmds.lines() {
                let mut p = line.splitn(3, '|');
                let (Some(i), Some(op), val) = (p.next().and_then(|s| s.parse::<usize>().ok()), p.next(), p.next().unwrap_or("")) else { continue };
                let Some(slot) = e.media.get_mut(i) else { continue };
                match (op, slot.media.as_mut()) {
                    ("play", Some(m)) => m.play(),
                    ("pause", Some(m)) => m.pause(),
                    ("seek", Some(m)) => m.seek(val.parse().unwrap_or(0.0)),
                    ("volume", Some(m)) => m.set_volume(val.parse().unwrap_or(1.0)),
                    ("muted", Some(m)) => m.set_muted(val == "1"),
                    ("play", None) => slot.autoplay = true, // start as soon as data arrives
                    ("load", _) => {
                        if let Some(u) = dro_net::resolve(&page, val) { slot.media = None; slot.rx = Some(spawn_fetch(u, page.clone())); }
                    }
                    _ => {}
                }
            }

            // 2) finished downloads -> Media (engine thread)
            let mut opened: Vec<(usize, Result<Media, String>)> = vec![];
            for (i, s) in e.media.iter_mut().enumerate() {
                let done = s.rx.as_ref().and_then(|rx| rx.try_recv().ok());
                if let Some(r) = done {
                    s.rx = None;
                    opened.push((i, r.and_then(|(b, hint)| Media::open(b, &hint))));
                }
            }
            for (i, r) in opened {
                match r {
                    Ok(mut m) => {
                        m.looping = m.looping || e.media[i].looping;
                        if let Some(v) = &m.video {
                            if let Some(pm) = Pixmap::new(v.width.max(1), v.height.max(1)) {
                                let mut pm = pm;
                                pm.fill(tiny_skia::Color::BLACK);
                                e.images.insert(format!("media:{i}"), pm);
                                e.layout = None; // size known now
                            }
                        }
                        if e.media[i].autoplay { m.play(); }
                        e.media[i].media = Some(m);
                        e.js_media_event(i, "loadedmetadata");
                        e.js_media_event(i, "canplay");
                        redraw = true;
                    }
                    Err(_) => e.js_media_event(i, "error"),
                }
            }

            // 3) state -> JS, frames -> images, events
            for i in 0..e.media.len() {
                let Some(m) = e.media[i].media.as_ref() else { continue };
                let (paused, t, d, vol, ended, muted) = (m.paused(), m.current_time(), m.duration, m.volume(), m.ended(), m.muted());
                if !paused { redraw = true; }
                let was = e.media[i].was_paused;
                let state = format!("__mediaState({i},{},{},{},{},{},{})", paused as u8, t, if d.is_nan() { "NaN".to_string() } else { d.to_string() }, vol, muted as u8, ended as u8);
                let mut events: Vec<&str> = vec![];
                if was && !paused { events.push("play"); }
                if !was && paused && !ended { events.push("pause"); }
                if !paused && t - e.media[i].last_time_event >= 0.25 { events.push("timeupdate"); e.media[i].last_time_event = t; }
                if ended && !e.media[i].ended_sent { events.push("ended"); e.media[i].ended_sent = true; }
                if !ended { e.media[i].ended_sent = false; }
                e.media[i].was_paused = paused;
                if let Some(pm) = e.media[i].media.as_ref().and_then(|m| m.frame()).filter(|f| f as *const _ as usize != e.media[i].frame_key) {
                    let key = pm as *const _ as usize;
                    if let Some(img) = premultiplied(pm) { e.images.insert(format!("media:{i}"), img); redraw = true; }
                    e.media[i].frame_key = key;
                }
                if let Some(js) = e.js.as_mut() { let _ = js.eval(&state); }
                for ev in events { e.js_media_event(i, ev); }
            }
            Ok(redraw)
        })
    }

    fn media_boxes(&mut self) -> Vec<(usize, f32, f32, f32, f32)> {
        let _ = self.ensure_layout();
        let Some(l) = self.layout.as_ref() else { return vec![] };
        l.items.iter().filter_map(|it| match &it.paint {
            lay::Paint::Image { src, .. } => src.strip_prefix("media:").and_then(|n| n.parse::<usize>().ok()).map(|i| (i, it.x, it.y, it.w, it.h)),
            _ => None,
        }).collect()
    }

    /// Draw control bars on top of a rendered frame (viewport pixels). No media -> returns input untouched.
    pub(super) fn overlay_media(&mut self, px: Vec<u8>) -> Vec<u8> {
        if self.media.is_empty() { return px; }
        let boxes = self.media_boxes();
        let (w, h) = self.vp;
        let Some(size) = tiny_skia::IntSize::from_wh(w, h) else { return px };
        let Some(mut pm) = Pixmap::from_vec(px.clone(), size) else { return px };
        let sy = self.scroll;
        for (i, x, y, bw, bh) in boxes {
            let Some(s) = self.media.get(i) else { continue };
            if !s.controls { continue; }
            let Some(m) = s.media.as_ref() else { continue };
            let st = ui::State { playing: !m.paused(), time: m.current_time(), duration: m.duration, volume: m.volume(), muted: m.muted() };
            ui::draw(&mut pm, x, y - sy, bw, bh, &st);
        }
        pm.take()
    }

    /// Click in viewport coordinates. Returns true if a media control handled it (then skip link hit-test).
    pub fn media_click(&mut self, x: f32, y: f32) -> bool {
        let boxes = self.media_boxes();
        let doc_y = y + self.scroll;
        for (i, bx, by, bw, bh) in boxes {
            let Some(s) = self.media.get_mut(i) else { continue };
            if !s.controls || x < bx || x > bx + bw || doc_y < by || doc_y > by + bh { continue; }
            let Some(m) = s.media.as_mut() else { return true };
            match ui::hit(x - bx, doc_y - by, bw, bh, m.duration) {
                Some(ui::Action::PlayPause) => { if m.paused() { m.play() } else { m.pause() } }
                Some(ui::Action::Seek(t)) => m.seek(t),
                Some(ui::Action::Volume(v)) => m.set_volume(v),
                Some(ui::Action::Mute) => { let mu = m.muted(); m.set_muted(!mu) }
                None => {}
            }
            return true;
        }
        false
    }

    /// Keyboard for the first loaded media: " " play/pause, ArrowLeft/Right +-5s, ArrowUp/Down volume.
    pub fn media_key(&mut self, key: &str) -> bool {
        let Some(m) = self.media.iter_mut().filter_map(|s| s.media.as_mut()).next() else { return false };
        match key {
            " " | "k" => { if m.paused() { m.play() } else { m.pause() } }
            "ArrowLeft" => { let t = m.current_time() - 5.0; m.seek(t) }
            "ArrowRight" => { let t = m.current_time() + 5.0; m.seek(t) }
            "ArrowUp" => { let v = m.volume() + 0.1; m.set_volume(v) }
            "ArrowDown" => { let v = m.volume() - 0.1; m.set_volume(v) }
            _ => return false,
        }
        true
    }
}
