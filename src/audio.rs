//! Sound card I/O at EchoLink's 8 kHz mono, resampling when the device
//! can't run at 8 kHz itself.

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream, StreamConfig, SupportedStreamConfigRange};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub const RATE: u32 = 8000;
/// The microphone chain (gain, noise reduction, auto level) runs at 48 kHz,
/// which is what the RNNoise model expects; it's brought down to 8 kHz last.
const WORK: u32 = 48000;
/// Most audio queued for playback before old audio is dropped (0.6 s).
const MAX_PLAYBACK: usize = RATE as usize * 6 / 10;
/// Audio buffered before playback starts, to ride out network jitter (0.24 s).
const PREBUFFER: usize = RATE as usize * 24 / 100;

/// Low-pass FIR (windowed sinc) that keeps the 300-3400 Hz voice band.
struct LowPass {
    taps: Vec<f32>,
    hist: VecDeque<f32>,
}

impl LowPass {
    fn new(rate: u32, cutoff: f32) -> Self {
        let n = 63;
        let fc = cutoff / rate as f32;
        let m = (n - 1) as f32 / 2.0;
        let mut taps: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f32 - m;
                let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f32::consts::PI * fc * x).sin() / (std::f32::consts::PI * x) };
                let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
                sinc * w
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= sum);
        LowPass { hist: VecDeque::from(vec![0.0; n]), taps }
    }

    fn push(&mut self, x: f32) -> f32 {
        self.hist.pop_front();
        self.hist.push_back(x);
        self.hist.iter().zip(&self.taps).map(|(a, b)| a * b).sum()
    }
}

/// Linear-interpolating rate converter with a low-pass on the high-rate side.
struct Resampler {
    step: f64, // input samples per output sample
    pos: f64,
    prev: f32,
    filter: Option<LowPass>,
    filter_input: bool,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        let high = from.max(to);
        Resampler {
            step: from as f64 / to as f64,
            pos: 0.0,
            prev: 0.0,
            filter: (from != to).then(|| LowPass::new(high, 3600.0)),
            filter_input: from > to,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.filter.is_none() {
            out.extend_from_slice(input);
            return;
        }
        for &raw in input {
            let x = match (&mut self.filter, self.filter_input) {
                (Some(f), true) => f.push(raw),
                _ => raw,
            };
            // Emit every output sample that falls between prev and x.
            while self.pos < 1.0 {
                let y = self.prev + (x - self.prev) * self.pos as f32;
                out.push(match (&mut self.filter, self.filter_input) {
                    (Some(f), false) => f.push(y),
                    _ => y,
                });
                self.pos += self.step;
            }
            self.pos -= 1.0;
            self.prev = x;
        }
    }
}

/// Level the auto-set and auto-level aim speech peaks at (0..1 of full scale).
/// The desktop meter turns yellow at 0.65 and red at 0.85.
pub const TARGET_PEAK: f32 = 0.70;
/// Peaks at or above this count as clipping.
pub const CLIP_LEVEL: f32 = 0.98;
pub const MAX_VOLUME: f32 = 2.0;
pub const GAIN_MIN_DB: f32 = -20.0;
pub const GAIN_MAX_DB: f32 = 40.0;
/// How long auto-set listens to you talk.
pub const CAL_SECONDS: f32 = 5.0;
/// Quieter than this (about -50 dBFS) means the mic isn't picking anything up.
const SILENCE: f32 = 0.003;

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Progress of the auto-set routine.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum CalState {
    #[default]
    Idle,
    /// Listening; 0..1 of the way through.
    Listening(f32),
    /// Finished; the new mic gain in dB.
    Done(f32),
    /// Heard (almost) nothing: muted, wrong input, or not talking.
    TooQuiet,
}

/// Listens to the raw microphone while you talk and works out the gain that
/// puts your speech peaks at `TARGET_PEAK`.
#[derive(Default)]
struct Calibrator {
    active: bool,
    peaks: Vec<f32>,
    block_peak: f32,
    block_n: usize,
    elapsed: usize,
    state: CalState,
}

impl Calibrator {
    fn start(&mut self) {
        *self = Calibrator { active: true, state: CalState::Listening(0.0), ..Default::default() };
    }

    /// Feed raw (pre-gain) mono samples. Returns the new gain when done.
    fn feed(&mut self, mono: &[f32], rate: u32) -> Option<f32> {
        if !self.active {
            return None;
        }
        let block = (rate / 100).max(1) as usize; // 10 ms
        for &x in mono {
            self.block_peak = self.block_peak.max(x.abs());
            self.block_n += 1;
            if self.block_n == block {
                self.peaks.push(self.block_peak);
                self.block_peak = 0.0;
                self.block_n = 0;
            }
        }
        self.elapsed += mono.len();
        let total = (rate as f32 * CAL_SECONDS) as usize;
        if self.elapsed < total {
            self.state = CalState::Listening(self.elapsed as f32 / total as f32);
            return None;
        }
        self.active = false;
        self.state = gain_for(&mut self.peaks);
        match self.state {
            CalState::Done(db) => Some(db),
            _ => None,
        }
    }
}

/// The gain that brings the loud end of `peaks` (10 ms block peaks, raw) to
/// `TARGET_PEAK`. The 98th percentile ignores the odd thump or cough.
fn gain_for(peaks: &mut [f32]) -> CalState {
    if peaks.is_empty() {
        return CalState::TooQuiet;
    }
    peaks.sort_by(|a, b| a.total_cmp(b));
    let p = peaks[((peaks.len() - 1) as f32 * 0.98) as usize];
    if p < SILENCE {
        return CalState::TooQuiet;
    }
    CalState::Done((20.0 * (TARGET_PEAK / p).log10()).clamp(GAIN_MIN_DB, GAIN_MAX_DB))
}

/// Room-noise remover: RNNoise (Xiph.org, BSD-3-Clause) through the
/// `nnnoiseless` crate. Works on 10 ms frames at 48 kHz, so it adds 10 ms of delay.
struct Denoiser {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    input: Vec<f32>,
    output: VecDeque<f32>,
    first: bool,
}

impl Denoiser {
    fn new() -> Denoiser {
        Denoiser {
            state: nnnoiseless::DenoiseState::new(),
            input: Vec::with_capacity(nnnoiseless::FRAME_SIZE),
            // One frame of lead-in so there's always as much output as input.
            output: VecDeque::from(vec![0.0; nnnoiseless::FRAME_SIZE]),
            first: true,
        }
    }

    /// Clean `buf` in place (samples in -1..1).
    fn process(&mut self, buf: &mut [f32]) {
        for &x in buf.iter() {
            self.input.push(x * 32768.0); // the model works on 16-bit-sized numbers
            if self.input.len() == nnnoiseless::FRAME_SIZE {
                let mut out = [0.0f32; nnnoiseless::FRAME_SIZE];
                self.state.process_frame(&mut out, &self.input);
                self.input.clear();
                if self.first {
                    out.fill(0.0); // the first frame out is a fade-in artefact
                    self.first = false;
                }
                self.output.extend(out.iter().map(|v| v / 32768.0));
            }
        }
        for s in buf.iter_mut() {
            *s = self.output.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
        }
    }
}

/// Slow automatic gain control with a soft limiter: lifts quiet talkers up to
/// `TARGET_PEAK`, pulls loud ones down fast, holds still when nobody is talking
/// (so it doesn't turn the room noise up), and never lets a peak past the red.
struct Agc {
    gain: f32,
    env: f32,
    decay: f32,
    attack: f32,
    rise: f32,
}

/// Below this input level (about -40 dBFS) the AGC holds its gain.
const AGC_GATE: f32 = 0.01;
const AGC_MAX_GAIN: f32 = 30.0; // about +30 dB
/// The limiter never lets the output pass this.
const LIMIT: f32 = 0.84;

impl Agc {
    fn new(rate: u32) -> Agc {
        let r = rate as f32;
        Agc {
            gain: 1.0,
            env: 0.0,
            decay: (-1.0 / (0.3 * r)).exp(),
            attack: 1.0 - (-1.0 / (0.002 * r)).exp(),
            rise: db_to_lin(6.0 / r), // +6 dB per second
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        for s in buf {
            let x = *s;
            self.env = x.abs().max(self.env * self.decay);
            let level = x.abs() * self.gain;
            if level > TARGET_PEAK {
                self.gain *= 1.0 - (1.0 - TARGET_PEAK / level) * self.attack;
            } else if self.env > AGC_GATE && self.env * self.gain < TARGET_PEAK * 0.9 {
                self.gain = (self.gain * self.rise).min(AGC_MAX_GAIN);
            }
            *s = soft_limit(x * self.gain);
        }
    }
}

/// Transparent below 0.7, then rounds off smoothly toward `LIMIT`.
fn soft_limit(y: f32) -> f32 {
    const KNEE: f32 = 0.7;
    let a = y.abs();
    if a <= KNEE {
        y
    } else {
        y.signum() * (KNEE + (LIMIT - KNEE) * ((a - KNEE) / (LIMIT - KNEE)).tanh())
    }
}

/// Shared state between the audio callbacks and the rest of the app.
#[derive(Default)]
pub struct Shared {
    playback: Mutex<VecDeque<i16>>,
    playing: AtomicBool,
    pub rx_level: AtomicU32,
    /// Level actually sent while transmitting (after gain), 0 otherwise.
    pub tx_level: AtomicU32,
    /// Live microphone level after gain, whether or not you're transmitting,
    /// so you can set the level before going on the air. Same 0..1000 scale.
    pub mic_level: AtomicU32,
    /// Counts blocks that hit full scale. Watch for it changing.
    pub clips: AtomicU32,
    /// Mic gain in tenths of a dB.
    gain_x10: AtomicI32,
    /// Automatic gain control and limiter on.
    pub auto_gain: AtomicBool,
    /// Background (room) noise removal on.
    pub denoise: AtomicBool,
    /// Speaker volume in percent (100 = as received).
    pub volume_x100: AtomicU32,
    /// Keep the microphone out of the transmission (while DTMF tones play).
    pub mute_mic: AtomicBool,
    /// How much quieter the noise reduction is making the mic, in tenths of a dB.
    pub nr_db_x10: AtomicU32,
    /// How far Auto level is pulling the peaks down, in tenths of a dB.
    pub comp_db_x10: AtomicU32,
    /// Connection health, measured from the other station's packets (see `link`):
    /// lost packets in tenths of a percent, jitter in tenths of a millisecond,
    /// milliseconds since anything came from them, and since a voice packet did
    /// (`u32::MAX` if none yet on this connection).
    pub link_loss_permille: AtomicU32,
    pub link_jitter_x10: AtomicU32,
    pub link_age_ms: AtomicU32,
    pub link_voice_age_ms: AtomicU32,
    cal: Mutex<Calibrator>,
}

impl Shared {
    pub fn volume(&self) -> f32 {
        self.volume_x100.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn set_volume(&self, v: f32) {
        self.volume_x100.store((v.clamp(0.0, MAX_VOLUME) * 100.0).round() as u32, Ordering::Relaxed);
    }

    pub fn gain_db(&self) -> f32 {
        self.gain_x10.load(Ordering::Relaxed) as f32 / 10.0
    }

    pub fn set_gain_db(&self, db: f32) {
        self.gain_x10.store((db.clamp(GAIN_MIN_DB, GAIN_MAX_DB) * 10.0).round() as i32, Ordering::Relaxed);
    }

    /// Listen for `CAL_SECONDS` and set the mic gain from how loud you talk.
    pub fn start_calibration(&self) {
        self.cal.lock().unwrap().start();
    }

    pub fn cancel_calibration(&self) {
        *self.cal.lock().unwrap() = Calibrator::default();
    }

    pub fn calibration(&self) -> CalState {
        self.cal.lock().unwrap().state
    }

    /// The outcome of a finished auto-set, once; `None` while idle or still listening.
    pub fn take_calibration_result(&self) -> Option<CalState> {
        let mut c = self.cal.lock().unwrap();
        match c.state {
            CalState::Done(_) | CalState::TooQuiet => Some(std::mem::take(&mut c.state)),
            _ => None,
        }
    }

    pub fn play(&self, pcm: &[i16]) {
        let mut q = self.playback.lock().unwrap();
        q.extend(pcm);
        let excess = q.len().saturating_sub(MAX_PLAYBACK);
        q.drain(..excess);
    }

    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.playback.lock().unwrap().len()
    }

    pub fn flush(&self) {
        self.playback.lock().unwrap().clear();
        self.playing.store(false, Ordering::Relaxed);
    }
}

fn peak(samples: impl Iterator<Item = f32>) -> u32 {
    (samples.fold(0.0f32, |m, s| m.max(s.abs())).min(1.0) * 1000.0) as u32
}

fn pick(ranges: Vec<SupportedStreamConfigRange>) -> Result<(StreamConfig, SampleFormat)> {
    let usable = |r: &&SupportedStreamConfigRange| matches!(r.sample_format(), SampleFormat::F32 | SampleFormat::I16);
    let best = ranges
        .iter()
        .filter(usable)
        .min_by_key(|r| {
            let fits = r.min_sample_rate().0 <= RATE && RATE <= r.max_sample_rate().0;
            (!fits, r.channels() != 1, r.sample_format() != SampleFormat::F32)
        })
        .ok_or_else(|| anyhow!("no f32 or i16 audio format available"))?;
    let rate = if best.min_sample_rate().0 <= RATE && RATE <= best.max_sample_rate().0 {
        RATE
    } else if best.min_sample_rate().0 <= 48000 && 48000 <= best.max_sample_rate().0 {
        48000
    } else {
        best.max_sample_rate().0
    };
    let cfg = best.with_sample_rate(SampleRate(rate));
    Ok((cfg.config(), cfg.sample_format()))
}

/// Names of the sound inputs and outputs on this computer.
pub fn devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |it: Result<Vec<cpal::Device>, _>| -> Vec<String> {
        let mut v: Vec<String> = it.unwrap_or_default().iter().filter_map(|d| d.name().ok()).collect();
        v.sort();
        v.dedup();
        v
    };
    (
        names(host.input_devices().map(|d| d.collect::<Vec<_>>())),
        names(host.output_devices().map(|d| d.collect::<Vec<_>>())),
    )
}

/// The named device, or the system default when `name` is empty or gone.
fn choose(
    host: &cpal::Host,
    name: Option<&str>,
    input: bool,
    notes: &mut Vec<String>,
) -> Result<cpal::Device> {
    if let Some(want) = name.map(str::trim).filter(|n| !n.is_empty()) {
        let mut all = if input { host.input_devices()? } else { host.output_devices()? };
        if let Some(d) = all.find(|d| d.name().is_ok_and(|n| n == want)) {
            return Ok(d);
        }
        notes.push(format!("{} \"{want}\" isn't connected; using the default.", if input { "Microphone" } else { "Speaker" }));
    }
    if input {
        host.default_input_device().context("no microphone found")
    } else {
        host.default_output_device().context("no speaker found")
    }
}

pub struct Audio {
    _input: Stream,
    _output: Stream,
    pub shared: Arc<Shared>,
    pub description: String,
    /// Things worth telling the user about how the devices were picked.
    pub notes: Vec<String>,
}

/// Open the default microphone and speaker. Captured audio is delivered to
/// `mic` in 8 kHz chunks while `transmitting` is set.
pub fn open(
    mic: Sender<Vec<i16>>,
    transmitting: Arc<AtomicBool>,
    input_name: Option<&str>,
    output_name: Option<&str>,
) -> Result<Audio> {
    let host = cpal::default_host();
    let shared = Arc::new(Shared::default());
    shared.volume_x100.store(100, Ordering::Relaxed);
    shared.link_voice_age_ms.store(u32::MAX, Ordering::Relaxed);
    let mut notes = Vec::new();

    let indev = choose(&host, input_name, true, &mut notes)?;
    let (incfg, infmt) = pick(indev.supported_input_configs()?.collect())?;
    let outdev = choose(&host, output_name, false, &mut notes)?;
    let (outcfg, outfmt) = pick(outdev.supported_output_configs()?.collect())?;

    let description = format!(
        "mic {} @ {} Hz, speaker {} @ {} Hz",
        indev.name().unwrap_or_default(),
        incfg.sample_rate.0,
        outdev.name().unwrap_or_default(),
        outcfg.sample_rate.0
    );

    // Capture: mix to mono, resample to 8 kHz, hand off while transmitting.
    let input = {
        let ch = incfg.channels as usize;
        let rate = incfg.sample_rate.0;
        let mut up = Resampler::new(rate, WORK);
        let mut rs = Resampler::new(WORK, RATE);
        let shared = shared.clone();
        let mut mono = Vec::new();
        let mut work = Vec::new();
        let mut out = Vec::new();
        let mut gain = 1.0f32;
        let smooth = 1.0 / (0.01 * WORK as f32); // gain changes glide over ~10 ms
        let mut agc = Agc::new(WORK);
        let mut denoiser: Option<Denoiser> = None;
        let (mut nr_smooth, mut comp_smooth) = (0.0f32, 0.0f32);
        let mut handle = move |data: &mut dyn Iterator<Item = f32>| {
            mono.clear();
            let mut acc = 0.0;
            for (i, s) in data.enumerate() {
                acc += s;
                if i % ch == ch - 1 {
                    mono.push(acc / ch as f32);
                    acc = 0.0;
                }
            }
            // Auto-set measures the raw mic, before any gain.
            if let Ok(mut cal) = shared.cal.try_lock() {
                if let Some(db) = cal.feed(&mono, rate) {
                    shared.set_gain_db(db);
                }
            }
            // From here on: 48 kHz.
            if rate != WORK {
                work.clear();
                up.process(&mono, &mut work);
                std::mem::swap(&mut mono, &mut work);
            }
            let target = db_to_lin(shared.gain_db());
            for s in mono.iter_mut() {
                gain += (target - gain) * smooth;
                *s *= gain;
            }
            // Noise removal goes before auto level, so the AGC never boosts room noise.
            let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt();
            let nr_target = if shared.denoise.load(Ordering::Relaxed) {
                let before = rms(&mono);
                denoiser.get_or_insert_with(Denoiser::new).process(&mut mono);
                let after = rms(&mono);
                // How much quieter it got; unchanged when there is nothing to measure.
                if before > 0.0005 { (20.0 * (before / after.max(1e-6)).log10()).clamp(0.0, 40.0) } else { nr_smooth }
            } else {
                denoiser = None;
                0.0
            };
            nr_smooth += (nr_target - nr_smooth) * 0.15;
            shared.nr_db_x10.store((nr_smooth * 10.0) as u32, Ordering::Relaxed);
            let comp_target = if shared.auto_gain.load(Ordering::Relaxed) {
                let before = peak(mono.iter().copied()) as f32 / 1000.0;
                agc.process(&mut mono);
                let after = peak(mono.iter().copied()) as f32 / 1000.0;
                // Only the pulling-down is shown; lifting quiet speech reads as no reduction.
                if before > 0.02 { (20.0 * (before / after.max(1e-6)).log10()).clamp(0.0, 30.0) } else { 0.0 }
            } else {
                agc.gain = 1.0; // start fresh next time it's switched on
                0.0
            };
            // Quick to rise, slow to fall, like a gain-reduction meter.
            comp_smooth += (comp_target - comp_smooth) * if comp_target > comp_smooth { 0.5 } else { 0.06 };
            shared.comp_db_x10.store((comp_smooth * 10.0) as u32, Ordering::Relaxed);
            let level = peak(mono.iter().copied());
            if level as f32 >= CLIP_LEVEL * 1000.0 {
                shared.clips.fetch_add(1, Ordering::Relaxed);
            }
            shared.mic_level.store(level, Ordering::Relaxed);
            let tx = transmitting.load(Ordering::Relaxed) && !shared.mute_mic.load(Ordering::Relaxed);
            shared.tx_level.store(if tx { level } else { 0 }, Ordering::Relaxed);
            if !tx {
                return;
            }
            out.clear();
            rs.process(&mono, &mut out);
            let pcm = out.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
            let _ = mic.send(pcm);
        };
        let err = |e| eprintln!("microphone: {e}");
        match infmt {
            SampleFormat::F32 => indev.build_input_stream(&incfg, move |d: &[f32], _: &_| handle(&mut d.iter().copied()), err, None)?,
            _ => indev.build_input_stream(
                &incfg,
                move |d: &[i16], _: &_| handle(&mut d.iter().map(|&s| s as f32 / 32768.0)),
                err,
                None,
            )?,
        }
    };

    // Playback: pull 8 kHz audio from the queue, resample, fan out to channels.
    let output = {
        let ch = outcfg.channels as usize;
        let mut rs = Resampler::new(RATE, outcfg.sample_rate.0);
        let ratio = outcfg.sample_rate.0 as f64 / RATE as f64;
        let shared = shared.clone();
        let mut pending: VecDeque<f32> = VecDeque::new();
        let mut src = Vec::new();
        let mut res = Vec::new();
        let mut fill = move |frames: usize| -> Vec<f32> {
            while pending.len() < frames {
                let need = ((frames - pending.len()) as f64 / ratio).ceil() as usize + 1;
                src.clear();
                {
                    let mut q = shared.playback.lock().unwrap();
                    if !shared.playing.load(Ordering::Relaxed) && q.len() >= PREBUFFER {
                        shared.playing.store(true, Ordering::Relaxed);
                    }
                    if shared.playing.load(Ordering::Relaxed) {
                        let take = need.min(q.len());
                        src.extend(q.drain(..take).map(|s| s as f32 / 32768.0));
                        if q.is_empty() {
                            shared.playing.store(false, Ordering::Relaxed);
                        }
                    }
                }
                src.resize(need, 0.0); // silence on underrun
                res.clear();
                rs.process(&src, &mut res);
                pending.extend(res.iter());
            }
            let vol = shared.volume();
            let block: Vec<f32> = pending.drain(..frames).map(|s| (s * vol).clamp(-1.0, 1.0)).collect();
            shared.rx_level.store(peak(block.iter().copied()), Ordering::Relaxed);
            block
        };
        let err = |e| eprintln!("speaker: {e}");
        match outfmt {
            SampleFormat::F32 => outdev.build_output_stream(
                &outcfg,
                move |d: &mut [f32], _: &_| {
                    let block = fill(d.len() / ch);
                    for (frame, s) in d.chunks_mut(ch).zip(block) {
                        frame.fill(s);
                    }
                },
                err,
                None,
            )?,
            _ => outdev.build_output_stream(
                &outcfg,
                move |d: &mut [i16], _: &_| {
                    let block = fill(d.len() / ch);
                    for (frame, s) in d.chunks_mut(ch).zip(block) {
                        frame.fill((s.clamp(-1.0, 1.0) * 32767.0) as i16);
                    }
                },
                err,
                None,
            )?,
        }
    };

    input.play()?;
    output.play()?;
    Ok(Audio { _input: input, _output: output, shared, description, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_ratios() {
        let input = vec![0.5f32; 48000];
        let mut out = Vec::new();
        Resampler::new(48000, 8000).process(&input, &mut out);
        assert!((out.len() as i64 - 8000).abs() <= 2, "{}", out.len());

        let input = vec![0.5f32; 8000];
        let mut out = Vec::new();
        Resampler::new(8000, 44100).process(&input, &mut out);
        assert!((out.len() as i64 - 44100).abs() <= 6, "{}", out.len());
        // DC passes through the filter at unity gain once it settles.
        assert!((out[40000] - 0.5).abs() < 0.01);
    }

    #[test]
    fn denoiser_quiets_room_noise_and_keeps_length() {
        // Fan-like rumble plus mains hum at about -32 dBFS: the kind of noise
        // RNNoise is trained for. (Flat white hiss is not; it can sound like speech.)
        let mut x = 7u32;
        let mut rand = move || {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            (x >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let (mut brown, mut n) = (0.0f32, 0usize);
        let mut room = move |len: usize| -> Vec<f32> {
            (0..len)
                .map(|_| {
                    brown = (brown + rand() * 0.06).clamp(-1.0, 1.0) * 0.999;
                    n += 1;
                    let hum = (n as f32 * 2.0 * std::f32::consts::PI * 120.0 / 48000.0).sin() * 0.3;
                    (brown * 2500.0 + hum * 250.0 + rand() * 120.0) / 32768.0
                })
                .collect()
        };
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        let mut d = Denoiser::new();
        let (mut tail_in, mut tail_out) = (0.0, 0.0);
        for block in 0..200 {
            let len = if block % 3 == 0 { 333 } else { 512 }; // odd sizes, like real callbacks
            let mut buf = room(len);
            let before = rms(&buf);
            d.process(&mut buf);
            assert_eq!(buf.len(), len);
            if block >= 150 {
                tail_in = before;
                tail_out = rms(&buf);
            }
        }
        // Measured about 40 dB; require at least 15.
        assert!(tail_out < tail_in * 0.18, "noise {tail_in} -> {tail_out}");
    }

    fn sine(amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.2).sin()).collect()
    }

    #[test]
    fn calibration_targets_the_yellow() {
        // Quiet talker: peaks around 0.1 should get lifted to the target.
        let mut peaks = vec![0.1; 400];
        peaks.extend(vec![0.01; 100]);
        match gain_for(&mut peaks) {
            CalState::Done(db) => {
                let landed = 0.1 * db_to_lin(db);
                assert!((landed - TARGET_PEAK).abs() < 0.02, "{landed}");
                assert!(landed > 0.65 && landed < 0.85, "should sit in the yellow, got {landed}");
            }
            other => panic!("{other:?}"),
        }
        // A loud talker gets turned down.
        match gain_for(&mut vec![0.95; 500]) {
            CalState::Done(db) => assert!(db < 0.0, "{db}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(gain_for(&mut vec![0.0005; 500]), CalState::TooQuiet);
        assert_eq!(gain_for(&mut []), CalState::TooQuiet);
    }

    #[test]
    fn calibrator_finishes_after_five_seconds() {
        let mut c = Calibrator::default();
        c.start();
        let chunk = sine(0.1, 480);
        let mut done = None;
        for _ in 0..(5 * 100 + 5) {
            done = done.or(c.feed(&chunk, 48000));
        }
        let db = done.expect("should have finished");
        assert!((0.1 * db_to_lin(db) - TARGET_PEAK).abs() < 0.03);
        assert!(!c.active);
    }

    #[test]
    fn agc_lifts_quiet_and_never_reaches_red() {
        let mut agc = Agc::new(8000);
        // Ten seconds of quiet speech-level tone: gain climbs toward the target.
        let mut quiet = sine(0.08, 80000);
        agc.process(&mut quiet);
        let tail = quiet[72000..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(tail > 0.5 && tail <= LIMIT, "tail {tail}");
        // Then it suddenly gets very loud: output stays out of the red from the first sample.
        let mut loud = sine(1.0, 8000);
        agc.process(&mut loud);
        assert!(loud.iter().all(|x| x.abs() <= LIMIT + 1e-4));
    }

    #[test]
    fn agc_holds_gain_in_silence() {
        let mut agc = Agc::new(8000);
        let mut noise = sine(0.002, 80000); // room noise, below the gate
        agc.process(&mut noise);
        assert!((agc.gain - 1.0).abs() < 1e-3, "{}", agc.gain);
    }
}
