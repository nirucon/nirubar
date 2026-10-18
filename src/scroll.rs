//! Smooth scrolling text for the bar.
//!
//! A title longer than the space it is given scrolls once, pauses, scrolls
//! again, and then rests showing the beginning: the part that says who and
//! what, which is what people look for when a track starts.
//!
//! The bar repaints on a timer, so the offset is derived from elapsed time
//! rather than kept as a counter. That makes the motion smooth and independent
//! of how often a repaint happens.

use std::time::{Duration, Instant};

/// One scrolling line.
pub struct Scroller {
    /// Characters shown in the window.
    visible: usize,
    /// Pause at the start before the first move.
    lead_in: Duration,
    /// Pause at each end of a pass.
    hold: Duration,
    /// Pause between the two passes.
    gap: Duration,
    /// How long one character takes to advance.
    per_char: Duration,
    /// How many full passes before it rests.
    passes: u32,
    started: Instant,
    /// The text last seen, so a new track starts from the beginning.
    current: String,
}

impl Scroller {
    pub fn new(visible: usize) -> Self {
        Self {
            visible: visible.max(4),
            lead_in: Duration::from_secs(2),
            hold: Duration::from_millis(1500),
            gap: Duration::from_millis(400),
            // 33 ms per character is roughly one column a frame at 30 Hz. The
            // old 120 ms moved the window by a third of a column per frame,
            // which on a thin bar reads as frozen rather than slow.
            per_char: Duration::from_millis(33),
            passes: 2,
            started: Instant::now(),
            current: String::new(),
        }
    }

    /// Whether the text is moving right now: not during the opening pause and
    /// not once the passes are over. The bar repaints at the frame rate only
    /// while this is true, and stays idle otherwise.
    pub fn moving(&self) -> bool {
        let chars = self.current.chars().count();
        if chars <= self.visible {
            return false;
        }
        let travel = (chars - self.visible) as f32;
        let step = self.per_char.as_secs_f32().max(0.001);
        let pass = self.lead_in.as_secs_f32() + travel * step + self.hold.as_secs_f32();
        let full = self.passes as f32 * pass + (self.passes as f32 - 1.0) * self.gap.as_secs_f32();
        let t = self.started.elapsed().as_secs_f32();
        if t >= full {
            return false;
        }
        let pass_len = pass + self.gap.as_secs_f32();
        let in_pass = t % pass_len;
        in_pass >= self.lead_in.as_secs_f32()
            && in_pass < self.lead_in.as_secs_f32() + travel * step
    }

    /// Tell the scroller what the player reports. A changed title restarts.
    pub fn observe(&mut self, text: &str) {
        if text != self.current {
            self.current = text.to_string();
            self.started = Instant::now();
        }
    }

    /// Whether this text needs scrolling at all.
    pub fn needs_scroll(&self, text: &str) -> bool {
        text.chars().count() > self.visible
    }

    /// The window of text to draw now.
    ///
    /// `elapsed` is taken from the clock on each call, so the bar can repaint
    /// at any rate and the motion stays even.
    pub fn frame(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= self.visible {
            return text.to_string();
        }
        let travel = chars.len() - self.visible;
        let step = self.per_char.as_secs_f32().max(0.001);

        // One pass: lead-in, travel, hold at the end.
        let pass = self.lead_in.as_secs_f32() + travel as f32 * step + self.hold.as_secs_f32();
        let full = self.passes as f32 * pass + (self.passes as f32 - 1.0) * self.gap.as_secs_f32();
        let t = self.started.elapsed().as_secs_f32();

        // After the passes, rest at the beginning: the artist stays visible.
        if t >= full {
            return chars[..self.visible].iter().collect();
        }
        let pass_len = pass + self.gap.as_secs_f32();
        let in_pass = t % pass_len;
        let offset = if in_pass < self.lead_in.as_secs_f32() {
            0.0
        } else {
            let moving = in_pass - self.lead_in.as_secs_f32();
            let moved = moving / step;
            moved.min(travel as f32)
        };
        let start = offset.round() as usize;
        chars[start..start + self.visible].iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short_scroller(visible: usize) -> Scroller {
        let mut s = Scroller::new(visible);
        s.lead_in = Duration::from_millis(0);
        s.hold = Duration::from_millis(0);
        s.gap = Duration::from_millis(0);
        s.per_char = Duration::from_millis(10);
        s
    }

    #[test]
    fn short_text_is_not_scrolled() {
        let s = Scroller::new(10);
        assert_eq!(s.frame("short"), "short");
        assert!(!s.needs_scroll("short"));
    }

    #[test]
    fn long_text_starts_from_the_beginning() {
        let mut s = short_scroller(5);
        s.observe("abcdefghij");
        assert_eq!(s.frame("abcdefghij"), "abcde");
    }

    #[test]
    fn long_text_moves_over_time() {
        let mut s = short_scroller(5);
        s.observe("abcdefghij");
        s.started -= Duration::from_millis(60);
        let frame = s.frame("abcdefghij");
        assert_ne!(frame, "abcde", "the window should have moved");
        assert_eq!(frame.chars().count(), 5);
    }

    #[test]
    fn it_rests_at_the_beginning_after_the_passes() {
        let mut s = short_scroller(5);
        s.observe("abcdefghij");
        // Far beyond the two passes.
        s.started -= Duration::from_secs(30);
        assert_eq!(s.frame("abcdefghij"), "abcde");
    }

    #[test]
    fn only_a_new_track_restarts_the_scroll() {
        // The bar renders the module once per frame (twice before 0.14.2).
        // Calling observe with the same track must not reset the clock, or the
        // window never moves past its first step.
        let mut s = short_scroller(5);
        s.observe("abcdefghij");
        s.started -= Duration::from_millis(60);
        let before = s.frame("abcdefghij");
        for _ in 0..5 {
            s.observe("abcdefghij");
        }
        assert_eq!(s.frame("abcdefghij"), before, "the same track is not news");
        // A different track is, and starts from the beginning.
        s.observe("klmnopqrst");
        assert_eq!(s.frame("klmnopqrst"), "klmno");
    }

    #[test]
    fn a_new_track_restarts_the_scroll() {
        let mut s = short_scroller(5);
        s.observe("abcdefghij");
        s.started -= Duration::from_secs(5);
        s.observe("klmnopqrst");
        assert_eq!(s.frame("klmnopqrst"), "klmno");
    }

    #[test]
    fn unicode_is_counted_in_characters() {
        let s = Scroller::new(4);
        // Four characters, not eight bytes.
        assert!(!s.needs_scroll("åäöé"));
    }
}
