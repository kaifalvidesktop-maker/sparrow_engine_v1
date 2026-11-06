//! Pure-Rust media player (no FFmpeg, no C).
//! Audio : symphonia (mp3/aac/flac/vorbis/ogg/wav/mp4-audio/mkv-audio) -> linear resampler -> cpal output.
//! Clock : audio clock is master (like Chromium). No-audio media uses a wall clock.
//! Video : animated GIF decoded in pure Rust; MP4 video is probed (size/fps/codec) but needs a decoder backend
//!         (AV1 via rav1d is the planned pure-Rust one). Audio of MP4 files plays today.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

pub struct Frame { pub w: u32, pub h: u32, pub rgba: Vec<u8> }

#[derive(Clone, Debug)]
pub struct VideoInfo { pub width: u32, pub height: u32, pub fps: f64, pub codec: String, pub duration: f64 }

struct Ring { buf: VecDeque<f32>, played: u64, base: f64 }

struct Shared {
    ring: Mutex<Ring>,
    playing: AtomicBool,
    muted: AtomicBool,
    volume: AtomicU32, // f32 bits
    ended: AtomicBool,
    done: AtomicBool, // decoder reached EOF
    stop: AtomicBool,
    seek: Mutex<Option<f64>>,
    rate: u32,
    ch: usize,
}

struct Wall { base: f64, since: Option<Instant> }
impl Wall {
    fn now(&self) -> f64 { self.base + self.since.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0) }
}

struct Gif { frames: Vec<(Frame, f64)>, total: f64 } // (frame, start time)

pub struct Media {
    shared: Option<Arc<Shared>>,
    _stream: Option<cpal::Stream>,
    thread: Option<JoinHandle<()>>,
    wall: Wall,
    gif: Option<Gif>,
    pub duration: f64, // NaN if unknown (same as HTMLMediaElement.duration)
    pub video: Option<VideoInfo>,
    pub looping: bool,
}

// ---------------------------------------------------------------- helpers
fn convert_channels(src: &[f32], sc: usize, oc: usize) -> Vec<f32> {
    if sc == oc { return src.to_vec(); }
    let frames = src.len() / sc.max(1);
    let mut o = Vec::with_capacity(frames * oc);
    for f in 0..frames {
        let fr = &src[f * sc..f * sc + sc];
        for c in 0..oc {
            o.push(if sc == 1 { fr[0] } else if oc == 1 { fr.iter().sum::<f32>() / sc as f32 } else { fr[c % sc] });
        }
    }
    o
}

struct Resampler { step: f64, pos: f64, last: Vec<f32> }
impl Resampler {
    fn new(src: u32, out: u32) -> Self { Resampler { step: src as f64 / out as f64, pos: 0.0, last: vec![] } }
    fn process(&mut self, inp: &[f32], ch: usize) -> Vec<f32> {
        if (self.step - 1.0).abs() < 1e-6 || ch == 0 { return inp.to_vec(); }
        let n = inp.len() / ch;
        let mut out = Vec::with_capacity((n as f64 / self.step) as usize * ch + ch);
        while self.pos < n as f64 {
            let i = self.pos.floor() as isize;
            let f = (self.pos - self.pos.floor()) as f32;
            for c in 0..ch {
                let a = if i < 0 { self.last.get(c).copied().unwrap_or(0.0) } else { inp[i as usize * ch + c] };
                let bi = (i + 1) as usize;
                let b = if bi >= n { a } else { inp[bi * ch + c] };
                out.push(a + (b - a) * f);
            }
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if n > 0 { self.last = inp[(n - 1) * ch..n * ch].to_vec(); }
        out
    }
}

fn time_secs(t: Time) -> f64 { t.seconds as f64 + t.frac }

fn build_stream<T: SizedSample + FromSample<f32>>(dev: &cpal::Device, cfg: &cpal::StreamConfig, sh: Arc<Shared>) -> Result<cpal::Stream, String> {
    let ch = cfg.channels as usize;
    dev.build_output_stream(
        cfg,
        move |out: &mut [T], _| {
            let Ok(mut r) = sh.ring.lock() else { for s in out.iter_mut() { *s = T::EQUILIBRIUM; } return };
            let playing = sh.playing.load(SeqCst);
            let vol = if sh.muted.load(SeqCst) { 0.0 } else { f32::from_bits(sh.volume.load(SeqCst)) };
            for frame in out.chunks_mut(ch) {
                if playing && r.buf.len() >= ch {
                    for s in frame.iter_mut() { *s = T::from_sample(r.buf.pop_front().unwrap_or(0.0) * vol); }
                    r.played += 1;
                } else {
                    for s in frame.iter_mut() { *s = T::EQUILIBRIUM; }
                    if playing && sh.done.load(SeqCst) { sh.ended.store(true, SeqCst); }
                }
            }
        },
        |e| eprintln!("audio stream error: {e}"),
        None,
    ).map_err(|e| e.to_string())
}

/// MP4 video track info (size, fps, codec) with the pure-Rust `mp4` crate.
pub fn probe_mp4(bytes: &[u8]) -> Option<VideoInfo> {
    let rd = mp4::Mp4Reader::read_header(Cursor::new(bytes), bytes.len() as u64).ok()?;
    let dur = rd.duration().as_secs_f64();
    for t in rd.tracks().values() {
        if t.track_type().ok() == Some(mp4::TrackType::Video) {
            let codec = t.media_type().map(|m| format!("{m:?}")).unwrap_or_else(|_| "unknown".into());
            return Some(VideoInfo { width: t.width() as u32, height: t.height() as u32, fps: t.frame_rate(), codec, duration: dur });
        }
    }
    None
}

fn load_gif(bytes: Vec<u8>) -> Result<Gif, String> {
    use image::AnimationDecoder;
    let dec = image::codecs::gif::GifDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut frames = vec![];
    let mut t = 0.0;
    let mut px: u64 = 0;
    for f in dec.into_frames() {
        let f = f.map_err(|e| e.to_string())?;
        let (n, d) = f.delay().numer_denom_ms();
        let dt = (n as f64 / d.max(1) as f64 / 1000.0).max(0.02);
        let img = f.into_buffer();
        px += img.width() as u64 * img.height() as u64;
        if frames.len() >= 2000 || px > 200_000_000 { break; } // memory guard
        frames.push((Frame { w: img.width(), h: img.height(), rgba: img.into_raw() }, t));
        t += dt;
    }
    if frames.is_empty() { return Err("empty gif".into()); }
    Ok(Gif { frames, total: t })
}

// ---------------------------------------------------------------- Media
impl Media {
    /// `hint` = file extension or mime subtype ("mp3", "mp4", "ogg", ...), may be "".
    pub fn open(bytes: Vec<u8>, hint: &str) -> Result<Media, String> {
        if bytes.starts_with(b"GIF8") {
            let g = load_gif(bytes)?;
            let (w, h) = (g.frames[0].0.w, g.frames[0].0.h);
            return Ok(Media {
                shared: None, _stream: None, thread: None, wall: Wall { base: 0.0, since: None }, duration: g.total,
                video: Some(VideoInfo { width: w, height: h, fps: g.frames.len() as f64 / g.total.max(0.001), codec: "gif".into(), duration: g.total }),
                gif: Some(g), looping: true,
            });
        }
        let video = if bytes.len() > 8 && &bytes[4..8] == b"ftyp" { probe_mp4(&bytes) } else { None };
        let mut m = Self::open_audio(bytes, hint)?;
        m.video = video;
        Ok(m)
    }

    fn open_audio(bytes: Vec<u8>, hint: &str) -> Result<Media, String> {
        let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut h = Hint::new();
        if !hint.is_empty() { h.with_extension(hint); }
        let probed = symphonia::default::get_probe().format(&h, mss, &FormatOptions::default(), &MetadataOptions::default()).map_err(|e| format!("unsupported media: {e}"))?;
        let mut format = probed.format;
        let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("no audio track")?;
        let (track_id, params) = (track.id, track.codec_params.clone());
        let src_rate = params.sample_rate.ok_or("unknown sample rate")?;
        let src_ch = params.channels.map(|c| c.count()).unwrap_or(2).max(1);
        let duration = match (params.n_frames, params.time_base) { (Some(n), Some(tb)) => time_secs(tb.calc_time(n)), _ => f64::NAN };
        let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;

        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or("no audio output device")?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        let (out_rate, out_ch) = (cfg.sample_rate().0, cfg.channels() as usize);
        let sh = Arc::new(Shared {
            ring: Mutex::new(Ring { buf: VecDeque::new(), played: 0, base: 0.0 }),
            playing: AtomicBool::new(false), muted: AtomicBool::new(false), volume: AtomicU32::new(1.0f32.to_bits()),
            ended: AtomicBool::new(false), done: AtomicBool::new(false), stop: AtomicBool::new(false),
            seek: Mutex::new(None), rate: out_rate, ch: out_ch,
        });
        let sc = cfg.config();
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => build_stream::<f32>(&dev, &sc, sh.clone())?,
            cpal::SampleFormat::I16 => build_stream::<i16>(&dev, &sc, sh.clone())?,
            cpal::SampleFormat::U16 => build_stream::<u16>(&dev, &sc, sh.clone())?,
            f => return Err(format!("unsupported sample format {f:?}")),
        };
        stream.play().map_err(|e| e.to_string())?;

        let t = sh.clone();
        let thread = std::thread::spawn(move || {
            let mut rs = Resampler::new(src_rate, out_rate);
            let mut sbuf: Option<SampleBuffer<f32>> = None;
            while !t.stop.load(SeqCst) {
                let req = t.seek.lock().ok().and_then(|mut s| s.take());
                if let Some(secs) = req {
                    let time = Time::new(secs.max(0.0) as u64, secs.max(0.0).fract());
                    if format.seek(SeekMode::Coarse, SeekTo::Time { time, track_id: Some(track_id) }).is_ok() {
                        decoder.reset();
                        rs = Resampler::new(src_rate, out_rate);
                        if let Ok(mut r) = t.ring.lock() { r.buf.clear(); r.played = 0; r.base = secs; }
                        t.done.store(false, SeqCst);
                        t.ended.store(false, SeqCst);
                    }
                }
                let queued = t.ring.lock().map(|r| r.buf.len()).unwrap_or(usize::MAX);
                if t.done.load(SeqCst) || queued > t.rate as usize * t.ch { std::thread::sleep(Duration::from_millis(10)); continue; }
                let pkt = match format.next_packet() {
                    Ok(p) => p,
                    Err(SymErr::ResetRequired) => { decoder.reset(); continue }
                    Err(_) => { t.done.store(true, SeqCst); continue }
                };
                if pkt.track_id() != track_id { continue; }
                match decoder.decode(&pkt) {
                    Ok(buf) => {
                        if sbuf.as_ref().map_or(true, |s| s.capacity() < buf.capacity() * buf.spec().channels.count()) {
                            sbuf = Some(SampleBuffer::<f32>::new(buf.capacity() as u64, *buf.spec()));
                        }
                        if let Some(sb) = sbuf.as_mut() {
                            sb.copy_interleaved_ref(buf);
                            let ch = src_ch.min(sb.samples().len().max(1));
                            let samples = convert_channels(sb.samples(), ch, t.ch);
                            let samples = rs.process(&samples, t.ch);
                            if let Ok(mut r) = t.ring.lock() { r.buf.extend(samples); }
                        }
                    }
                    Err(SymErr::DecodeError(_)) => continue, // skip corrupt packet
                    Err(_) => { t.done.store(true, SeqCst); }
                }
            }
        });
        Ok(Media { shared: Some(sh), _stream: Some(stream), thread: Some(thread), wall: Wall { base: 0.0, since: None }, gif: None, duration, video: None, looping: false })
    }

    pub fn play(&mut self) {
        if let Some(s) = self.shared.clone() {
            if s.ended.load(SeqCst) { self.seek(0.0); }
            s.playing.store(true, SeqCst);
        } else if self.wall.since.is_none() { self.wall.since = Some(Instant::now()); }
    }
    pub fn pause(&mut self) {
        if let Some(s) = &self.shared { s.playing.store(false, SeqCst); }
        else { self.wall.base = self.wall.now(); self.wall.since = None; }
    }
    pub fn paused(&self) -> bool {
        match &self.shared { Some(s) => !s.playing.load(SeqCst), None => self.wall.since.is_none() }
    }
    pub fn ended(&self) -> bool { self.shared.as_ref().map_or(false, |s| s.ended.load(SeqCst)) }
    pub fn set_volume(&mut self, v: f32) { if let Some(s) = &self.shared { s.volume.store(v.clamp(0.0, 1.0).to_bits(), SeqCst); } }
    pub fn volume(&self) -> f32 { self.shared.as_ref().map_or(1.0, |s| f32::from_bits(s.volume.load(SeqCst))) }
    pub fn set_muted(&mut self, m: bool) { if let Some(s) = &self.shared { s.muted.store(m, SeqCst); } }
    pub fn muted(&self) -> bool { self.shared.as_ref().map_or(false, |s| s.muted.load(SeqCst)) }
    pub fn has_audio(&self) -> bool { self.shared.is_some() }

    pub fn seek(&mut self, secs: f64) {
        let secs = if self.duration.is_finite() { secs.clamp(0.0, self.duration) } else { secs.max(0.0) };
        match &self.shared {
            Some(s) => { if let Ok(mut q) = s.seek.lock() { *q = Some(secs); } }
            None => { self.wall.base = secs; if self.wall.since.is_some() { self.wall.since = Some(Instant::now()); } }
        }
    }

    /// Current playback position in seconds (audio clock when audio exists).
    pub fn current_time(&self) -> f64 {
        let t = match &self.shared {
            Some(s) => s.ring.lock().map(|r| r.base + r.played as f64 / s.rate as f64).unwrap_or(0.0),
            None => self.wall.now(),
        };
        if self.duration.is_finite() && !self.looping { t.min(self.duration) } else { t }
    }

    /// Frame to show right now (animated GIF). None for audio-only / undecodable video.
    pub fn frame(&self) -> Option<&Frame> {
        let g = self.gif.as_ref()?;
        let mut t = self.current_time();
        if self.looping && g.total > 0.0 { t %= g.total; }
        let i = g.frames.partition_point(|(_, start)| *start <= t).saturating_sub(1);
        g.frames.get(i).map(|(f, _)| f)
    }

    /// True when the video track can be shown (a decoder backend exists).
    pub fn video_supported(&self) -> bool { self.gif.is_some() }
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Some(s) = &self.shared { s.stop.store(true, SeqCst); }
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_conversion() {
        assert_eq!(convert_channels(&[1.0, 2.0], 1, 2), vec![1.0, 1.0, 2.0, 2.0]);
        assert_eq!(convert_channels(&[1.0, 3.0], 2, 1), vec![2.0]);
        assert_eq!(convert_channels(&[1.0, 2.0], 2, 2), vec![1.0, 2.0]);
    }
    #[test]
    fn resampler_ratio() {
        let mut r = Resampler::new(44100, 48000);
        let input = vec![0.5f32; 4410];
        let out = r.process(&input, 1);
        assert!((out.len() as i64 - 4800).abs() <= 2, "{}", out.len());
        assert!(out.iter().all(|v| (*v - 0.5).abs() < 1e-3));
    }
    #[test]
    fn wall_clock() {
        let w = Wall { base: 2.0, since: None };
        assert_eq!(w.now(), 2.0);
    }
    #[test]
    fn bad_input_is_error() {
        assert!(Media::open(vec![1, 2, 3, 4], "").is_err());
    }
}
