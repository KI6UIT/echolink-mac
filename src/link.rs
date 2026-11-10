//! How healthy the connection to the station you're linked to is, measured from the
//! voice packets they send: lost packets from gaps in the sequence numbers, jitter
//! from how unevenly the packets arrive, and how recently we heard from them at all.
//!
//! EchoLink has no ping, so true round-trip delay can't be measured.

/// Voice packets carry 80 ms of audio, so that is how far apart they should arrive.
const PACKET_MS: f32 = 80.0;
/// Further apart than this and it's a new over, not jitter.
const SAME_OVER_MS: u64 = 400;

#[derive(Default, Clone, Debug)]
pub struct LinkStats {
    last_seq: Option<u16>,
    last_at_ms: Option<u64>,
    recv: u32,
    lost: u32,
    jitter_ms: f32,
}

impl LinkStats {
    /// Note a voice packet with sequence number `seq` arriving at `at_ms`.
    pub fn packet(&mut self, seq: u16, at_ms: u64) {
        if let Some(prev) = self.last_seq {
            let gap = seq.wrapping_sub(prev.wrapping_add(1));
            // 0 = next in line. A small gap is packets that never came. A huge one is a
            // repeat, a late arrival, or the sender starting a new count: not loss.
            if gap > 0 && gap < 100 {
                self.lost += u32::from(gap);
            }
        }
        self.last_seq = Some(seq);
        self.recv += 1;
        if let Some(prev) = self.last_at_ms.replace(at_ms) {
            let gap = at_ms.saturating_sub(prev);
            if gap < SAME_OVER_MS {
                let d = (gap as f32 - PACKET_MS).abs();
                self.jitter_ms += (d - self.jitter_ms) / 16.0; // the usual running average
            }
        }
        // Keep roughly the last few hundred packets: halve the tallies when they grow.
        if self.recv + self.lost > 400 {
            self.recv /= 2;
            self.lost /= 2;
        }
    }

    /// Percent of packets that went missing recently.
    pub fn loss_pct(&self) -> f32 {
        let total = self.recv + self.lost;
        if total == 0 { 0.0 } else { self.lost as f32 * 100.0 / total as f32 }
    }

    pub fn jitter_ms(&self) -> f32 {
        self.jitter_ms
    }

    pub fn has_data(&self) -> bool {
        self.recv > 0
    }

    pub fn reset(&mut self) {
        *self = LinkStats::default();
    }
}

/// Link health from 0 (dead) to 1 (perfect).
///
/// * `age_secs`: how long since anything at all came from the other station. It sends
///   a keepalive every 10 s, so quiet is normal up to about 12 s; at 50 s the link times out.
/// * `loss_pct` and `jitter_ms` count only if `recent_voice` (they describe the last over,
///   and say nothing about a link that has gone quiet).
pub fn quality(age_secs: f32, loss_pct: f32, jitter_ms: f32, recent_voice: bool) -> f32 {
    let fresh = if age_secs <= 12.0 { 1.0 } else { ((50.0 - age_secs) / 38.0).clamp(0.0, 1.0) };
    let penalty = if recent_voice { loss_pct * 0.08 + (jitter_ms / 300.0).min(0.4) } else { 0.0 };
    ((1.0 - penalty).clamp(0.0, 1.0)) * fresh
}

pub fn grade(q: f32) -> &'static str {
    if q >= 0.8 {
        "GOOD"
    } else if q >= 0.5 {
        "FAIR"
    } else {
        "POOR"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_stream_has_no_loss_and_little_jitter() {
        let mut s = LinkStats::default();
        for i in 0..100u16 {
            s.packet(i, i as u64 * 80);
        }
        assert_eq!(s.loss_pct(), 0.0);
        assert!(s.jitter_ms() < 0.5);
        assert!(s.has_data());
    }

    #[test]
    fn missing_sequence_numbers_are_counted_as_loss() {
        let mut s = LinkStats::default();
        // Every tenth packet never arrives: 9 received, 1 lost per ten.
        let mut t = 0;
        for i in 0..100u16 {
            t += 80;
            if i % 10 != 5 {
                s.packet(i, t);
            }
        }
        assert!((s.loss_pct() - 10.0).abs() < 1.5, "{}", s.loss_pct());
    }

    #[test]
    fn sequence_numbers_wrap_and_resets_are_not_loss() {
        let mut s = LinkStats::default();
        for (n, seq) in [65533u16, 65534, 65535, 0, 1, 2].into_iter().enumerate() {
            s.packet(seq, n as u64 * 80);
        }
        assert_eq!(s.loss_pct(), 0.0, "wrapping around is fine");
        let mut s = LinkStats::default();
        s.packet(5000, 0);
        s.packet(7, 80); // the sender started counting again
        s.packet(5000, 160); // or a stray late one
        assert_eq!(s.loss_pct(), 0.0);
    }

    #[test]
    fn uneven_arrival_raises_jitter_and_pauses_between_overs_do_not() {
        let mut s = LinkStats::default();
        let mut t = 0;
        for i in 0..200u16 {
            t += if i % 2 == 0 { 40 } else { 120 }; // 40 ms off the beat each time
            s.packet(i, t);
        }
        assert!((s.jitter_ms() - 40.0).abs() < 6.0, "{}", s.jitter_ms());
        let before = s.jitter_ms();
        s.packet(200, t + 30_000); // a minute later is a new over
        assert!((s.jitter_ms() - before).abs() < 0.001);
    }

    #[test]
    fn tallies_stay_recent() {
        let mut s = LinkStats::default();
        for i in 0..2000u16 {
            if i < 1000 && i % 2 == 0 {
                continue; // heavy loss early on
            }
            s.packet(i, i as u64 * 80);
        }
        assert!(s.loss_pct() < 5.0, "old loss should have faded, got {}", s.loss_pct());
    }

    #[test]
    fn quality_rewards_fresh_clean_links() {
        assert_eq!(quality(3.0, 0.0, 0.0, true), 1.0);
        assert!(quality(3.0, 10.0, 0.0, true) < 0.3, "10% loss is bad");
        assert!(quality(3.0, 0.0, 400.0, true) <= 0.6 + 1e-6, "heavy jitter is capped but hurts");
        // A quiet link isn't judged on old loss figures, only on whether it's still alive.
        assert_eq!(quality(8.0, 30.0, 300.0, false), 1.0);
        assert!(quality(30.0, 0.0, 0.0, false) < 0.6);
        assert_eq!(quality(60.0, 0.0, 0.0, false), 0.0);
        assert_eq!((grade(0.95), grade(0.6), grade(0.2)), ("GOOD", "FAIR", "POOR"));
    }
}
