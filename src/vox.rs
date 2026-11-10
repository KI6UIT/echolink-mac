//! Voice-operated transmit: key up when you start talking, unkey a moment after you stop.
//!
//! The rules live here, apart from the sound card, so they can be tested with made-up levels.

/// What the engine should do about the microphone level it just saw.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Action {
    None,
    Key,
    Unkey,
}

#[derive(Default, Debug)]
pub struct Vox {
    last_loud_ms: Option<u64>,
    /// We keyed the transmitter (so we're the one to unkey it, not a person holding the key).
    keyed: bool,
    /// After a time-out or a hang-up: wait for a real pause before keying again, so a long
    /// talker isn't re-keyed the instant the 3-minute limit cuts them off.
    needs_quiet: bool,
}

impl Vox {
    /// Look at one chunk of microphone audio.
    ///
    /// * `level`: its peak, 0 to 1, after all the processing.
    /// * `can_key`: connected, and nobody is being heard (and the speaker has just fallen silent),
    ///   so the speaker can't set it off.
    /// * `transmitting`: the transmitter is on right now, whoever turned it on.
    pub fn update(&mut self, level: f32, threshold: f32, hang_ms: u64, now_ms: u64, can_key: bool, transmitting: bool) -> Action {
        let loud = level >= threshold;
        if loud {
            self.last_loud_ms = Some(now_ms);
        }
        let quiet_for = self.last_loud_ms.map(|t| now_ms.saturating_sub(t)).unwrap_or(u64::MAX);
        if self.needs_quiet && quiet_for >= hang_ms {
            self.needs_quiet = false;
        }
        if !transmitting {
            self.keyed = false;
            if loud && can_key && !self.needs_quiet {
                self.keyed = true;
                return Action::Key;
            }
            return Action::None;
        }
        if self.keyed && quiet_for >= hang_ms {
            self.keyed = false;
            return Action::Unkey;
        }
        Action::None
    }

    /// The transmit time limit just cut in (or the link dropped).
    pub fn timed_out(&mut self) {
        self.keyed = false;
        self.needs_quiet = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f32 = 0.1;

    #[test]
    fn keys_when_you_start_talking_and_unkeys_after_the_hang_time() {
        let mut v = Vox::default();
        assert_eq!(v.update(0.01, T, 700, 0, true, false), Action::None, "room noise doesn't key it");
        assert_eq!(v.update(0.3, T, 700, 10, true, false), Action::Key);
        // Now transmitting. Speech with short pauses keeps it keyed.
        assert_eq!(v.update(0.3, T, 700, 20, true, true), Action::None);
        assert_eq!(v.update(0.01, T, 700, 400, true, true), Action::None, "a pause shorter than the hang time");
        assert_eq!(v.update(0.3, T, 700, 500, true, true), Action::None);
        assert_eq!(v.update(0.01, T, 700, 1100, true, true), Action::None);
        assert_eq!(v.update(0.01, T, 700, 1250, true, true), Action::Unkey, "700 ms after the last loud chunk");
    }

    #[test]
    fn will_not_key_when_it_must_not() {
        let mut v = Vox::default();
        assert_eq!(v.update(0.9, T, 700, 0, false, false), Action::None, "someone is talking, or not connected");
        assert_eq!(v.update(0.9, T, 700, 10, true, false), Action::Key, "but it can once that's over");
    }

    #[test]
    fn leaves_a_key_you_are_holding_alone() {
        let mut v = Vox::default();
        // The transmitter is on (you're holding the talk key); VOX never keyed it.
        for t in 0..30 {
            assert_eq!(v.update(0.0, T, 700, t * 100, true, true), Action::None);
        }
    }

    #[test]
    fn after_a_time_out_it_waits_for_a_real_pause() {
        let mut v = Vox::default();
        assert_eq!(v.update(0.5, T, 700, 0, true, false), Action::Key);
        v.timed_out(); // the 3-minute limit unkeyed us mid-sentence
        assert_eq!(v.update(0.5, T, 700, 100, true, false), Action::None, "still talking: stay off");
        assert_eq!(v.update(0.5, T, 700, 600, true, false), Action::None);
        assert_eq!(v.update(0.0, T, 700, 1000, true, false), Action::None);
        assert_eq!(v.update(0.0, T, 700, 1400, true, false), Action::None, "quiet long enough: armed again");
        assert_eq!(v.update(0.5, T, 700, 1500, true, false), Action::Key);
    }
}
