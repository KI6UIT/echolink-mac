//! DTMF (touch-tone) generation. EchoLink carries DTMF as tones in the voice
//! stream, so a keypad press is just a short burst of audio at 8 kHz.

use crate::audio::RATE;

const TONE_MS: usize = 120;
const GAP_MS: usize = 80;
/// Each of the two tones; their sum peaks near 0.5 of full scale, which survives
/// the GSM codec cleanly.
const AMP: f32 = 0.25;

/// Row and column frequencies for a key, or `None` if it isn't a DTMF key.
pub fn freqs(key: char) -> Option<(f32, f32)> {
    const ROWS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
    const COLS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
    const KEYS: [[char; 4]; 4] = [['1', '2', '3', 'A'], ['4', '5', '6', 'B'], ['7', '8', '9', 'C'], ['*', '0', '#', 'D']];
    let key = key.to_ascii_uppercase();
    for (r, row) in KEYS.iter().enumerate() {
        if let Some(c) = row.iter().position(|&k| k == key) {
            return Some((ROWS[r], COLS[c]));
        }
    }
    None
}

/// True for the characters `pcm` can play.
pub fn valid(key: char) -> bool {
    freqs(key).is_some()
}

/// 8 kHz audio for `digits`: a tone for each key with a short gap between.
/// Characters that aren't DTMF keys are skipped.
pub fn pcm(digits: &str) -> Vec<i16> {
    let per_ms = RATE as usize / 1000;
    let mut out = Vec::new();
    for key in digits.chars() {
        let Some((lo, hi)) = freqs(key) else { continue };
        let n = TONE_MS * per_ms;
        for i in 0..n {
            let t = i as f32 / RATE as f32;
            let w = 2.0 * std::f32::consts::PI;
            let mut s = AMP * ((w * lo * t).sin() + (w * hi * t).sin());
            // 4 ms ramps so the burst doesn't click.
            let edge = (4 * per_ms) as f32;
            s *= (i as f32 / edge).min(1.0).min(((n - i) as f32 / edge).min(1.0));
            out.push((s * 32767.0) as i16);
        }
        out.extend(std::iter::repeat_n(0i16, GAP_MS * per_ms));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Energy of `freq` in `x` (Goertzel).
    fn power(x: &[i16], freq: f32) -> f32 {
        let k = 2.0 * (2.0 * std::f32::consts::PI * freq / RATE as f32).cos();
        let (mut a, mut b) = (0.0f32, 0.0f32);
        for &s in x {
            let c = s as f32 / 32768.0 + k * a - b;
            b = a;
            a = c;
        }
        (a * a + b * b - k * a * b) / x.len() as f32
    }

    fn decode(tone: &[i16]) -> char {
        let rows = [697.0, 770.0, 852.0, 941.0];
        let cols = [1209.0, 1336.0, 1477.0, 1633.0];
        let best = |f: [f32; 4]| (0..4).max_by(|&i, &j| power(tone, f[i]).total_cmp(&power(tone, f[j]))).unwrap();
        ['1', '2', '3', 'A', '4', '5', '6', 'B', '7', '8', '9', 'C', '*', '0', '#', 'D'][best(rows) * 4 + best(cols)]
    }

    #[test]
    fn every_key_decodes_back_to_itself() {
        let n = (TONE_MS + GAP_MS) * (RATE as usize / 1000);
        for key in "123A456B789C*0#D".chars() {
            let audio = pcm(&key.to_string());
            assert_eq!(audio.len(), n);
            assert_eq!(decode(&audio[200..(TONE_MS * 8) - 200]), key, "key {key}");
        }
    }

    #[test]
    fn skips_junk_and_stays_below_full_scale() {
        assert!(pcm("xyz ").is_empty());
        assert_eq!(pcm("1 2").len(), pcm("12").len());
        let peak = pcm("5#").iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak < 18000, "{peak}");
        assert!(valid('a') && !valid('e'));
    }
}
