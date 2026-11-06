//! Chromium-style media control bar: draw + hit-test. Coordinates are relative to the media box.
use crate::dro_font;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

pub const BAR_H: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action { PlayPause, Seek(f64), Volume(f32), Mute }

#[derive(Clone, Copy, Debug)]
pub struct State { pub playing: bool, pub time: f64, pub duration: f64, pub volume: f32, pub muted: bool }

struct L { play: (f32, f32), prog: (f32, f32), mute: (f32, f32), vol: (f32, f32) }

fn layout(w: f32) -> L {
    L { play: (0.0, 36.0), prog: (150.0, (w - 108.0).max(150.0)), mute: (w - 100.0, w - 68.0), vol: (w - 64.0, w - 8.0) }
}

pub fn fmt_time(s: f64) -> String {
    if !s.is_finite() || s < 0.0 { return "0:00".into(); }
    let t = s as u64;
    if t >= 3600 { format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60) } else { format!("{}:{:02}", t / 60, t % 60) }
}

/// `x`,`y` relative to the media box (w x h). Returns the action for a click on the bar.
pub fn hit(x: f32, y: f32, w: f32, h: f32, duration: f64) -> Option<Action> {
    if y < h - BAR_H || y > h || x < 0.0 || x > w { return None; }
    let l = layout(w);
    if x >= l.play.0 && x < l.play.1 { return Some(Action::PlayPause); }
    if x >= l.mute.0 && x < l.mute.1 { return Some(Action::Mute); }
    if x >= l.vol.0 && x <= l.vol.1 { return Some(Action::Volume(((x - l.vol.0) / (l.vol.1 - l.vol.0)).clamp(0.0, 1.0))); }
    if x >= l.prog.0 && x <= l.prog.1 && duration.is_finite() && duration > 0.0 {
        return Some(Action::Seek(((x - l.prog.0) / (l.prog.1 - l.prog.0)).clamp(0.0, 1.0) as f64 * duration));
    }
    None
}

fn fill(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: (u8, u8, u8, u8)) {
    if let Some(r) = Rect::from_xywh(x, y, w.max(0.0), h.max(0.0)) {
        let mut p = Paint::default();
        p.set_color_rgba8(c.0, c.1, c.2, c.3);
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
}

pub fn draw(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, s: &State) {
    let by = y + h - BAR_H;
    let l = layout(w);
    fill(pm, x, by, w, BAR_H, (20, 20, 20, 215));
    let white = (255u8, 255u8, 255u8, 255u8);
    // play / pause icon
    if s.playing {
        fill(pm, x + 11.0, by + 10.0, 5.0, 16.0, white);
        fill(pm, x + 20.0, by + 10.0, 5.0, 16.0, white);
    } else {
        let mut pb = PathBuilder::new();
        pb.move_to(x + 12.0, by + 9.0);
        pb.line_to(x + 12.0, by + 27.0);
        pb.line_to(x + 27.0, by + 18.0);
        pb.close();
        if let Some(p) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 255, 255, 255);
            paint.anti_alias = true;
            pm.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    // time text
    let txt = format!("{} / {}", fmt_time(s.time), if s.duration.is_finite() { fmt_time(s.duration) } else { "--:--".into() });
    dro_font::draw_text(pm, &txt, x + 42.0, by, BAR_H, 13.0, false, (255, 255, 255));
    // progress bar
    let pw = l.prog.1 - l.prog.0;
    fill(pm, x + l.prog.0, by + 16.0, pw, 4.0, (255, 255, 255, 70));
    if s.duration.is_finite() && s.duration > 0.0 {
        let f = (s.time / s.duration).clamp(0.0, 1.0) as f32;
        fill(pm, x + l.prog.0, by + 16.0, pw * f, 4.0, (66, 133, 244, 255));
        fill(pm, x + l.prog.0 + pw * f - 4.0, by + 13.0, 8.0, 10.0, white);
    }
    // speaker
    let m = if s.muted { (255, 120, 120, 255) } else { white };
    fill(pm, x + l.mute.0 + 8.0, by + 14.0, 6.0, 8.0, m);
    fill(pm, x + l.mute.0 + 14.0, by + 11.0, 4.0, 14.0, m);
    if s.muted { fill(pm, x + l.mute.0 + 21.0, by + 17.0, 8.0, 2.0, m); }
    // volume
    let vw = l.vol.1 - l.vol.0;
    fill(pm, x + l.vol.0, by + 16.0, vw, 4.0, (255, 255, 255, 70));
    let v = if s.muted { 0.0 } else { s.volume.clamp(0.0, 1.0) };
    fill(pm, x + l.vol.0, by + 16.0, vw * v, 4.0, white);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_format() {
        assert_eq!(fmt_time(5.0), "0:05");
        assert_eq!(fmt_time(125.0), "2:05");
        assert_eq!(fmt_time(3725.0), "1:02:05");
        assert_eq!(fmt_time(f64::NAN), "0:00");
    }
    #[test]
    fn hits() {
        let (w, h) = (640.0, 360.0);
        assert_eq!(hit(10.0, h - 10.0, w, h, 100.0), Some(Action::PlayPause));
        assert_eq!(hit(10.0, 50.0, w, h, 100.0), None); // not on the bar
        match hit(w / 2.0, h - 18.0, w, h, 100.0) { Some(Action::Seek(t)) => assert!(t > 30.0 && t < 70.0, "{t}"), o => panic!("{o:?}") }
        assert_eq!(hit(w - 85.0, h - 18.0, w, h, 100.0), Some(Action::Mute));
        assert!(matches!(hit(w - 30.0, h - 18.0, w, h, 100.0), Some(Action::Volume(_))));
        assert_eq!(hit(w / 2.0, h - 18.0, w, h, f64::NAN), None); // unknown duration: no seeking
    }
    #[test]
    fn draws_without_panic() {
        let mut pm = Pixmap::new(640, 360).expect("pm");
        pm.fill(tiny_skia::Color::BLACK);
        draw(&mut pm, 0.0, 0.0, 640.0, 360.0, &State { playing: true, time: 12.0, duration: 90.0, volume: 0.7, muted: false });
        draw(&mut pm, 0.0, 0.0, 640.0, 360.0, &State { playing: false, time: 0.0, duration: f64::NAN, volume: 1.0, muted: true });
        assert!(pm.data().iter().any(|&b| b > 100));
        let _ = pm.save_png("/tmp/media_ui.png");
    }
}
