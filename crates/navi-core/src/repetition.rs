/// Detects repetitive model output patterns: character runs ("aaaaa...")
/// and alternating 2-char patterns ("-_-_-_").
#[derive(Default)]
pub(crate) struct RepetitionDetector {
    // ─ character‑run detection ─
    last_char: Option<char>,
    run_length: usize,
    // ─ alternating‑pattern detection ─
    /// (prev_char, current_char) pair being tracked
    alt_pair: Option<(char, char)>,
    alt_count: usize,
    /// last 2 chars for detecting the start of a new cycle
    alt_buf: [char; 2],
    alt_buf_len: usize,
    /// How many characters have been classified as alternating so far this run
    alt_run_chars: usize,
}

/// Thresholds for repetition detection.
const MAX_CHAR_RUN: usize = 80;
const MAX_ALT_CYCLES: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepetitionKind {
    /// The model is outputting the same character repeatedly.
    CharRun { ch: char, count: usize },
    /// The model is outputting a repeating 2-char pattern.
    AlternatingPattern { pattern: String, cycles: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct RepetitionWarning {
    pub kind: RepetitionKind,
    pub message: String,
}

impl RepetitionDetector {
    pub fn feed_text(&mut self, text: &str) -> Option<RepetitionWarning> {
        for ch in text.chars() {
            self.feed_char(ch);
        }
        self.check_warnings()
    }

    pub fn feed_thinking(&mut self, text: &str) -> Option<RepetitionWarning> {
        for ch in text.chars() {
            self.feed_char(ch);
        }
        self.check_warnings()
    }

    fn feed_char(&mut self, ch: char) {
        // ─ character run ─
        if Some(ch) == self.last_char {
            self.run_length += 1;
        } else {
            self.last_char = Some(ch);
            self.run_length = 1;
        }

        // ─ alternating pattern detection ─
        // We track a 2-char sliding window and look for repeating (a,b) pairs.
        if self.alt_buf_len < 2 {
            self.alt_buf[self.alt_buf_len] = ch;
            self.alt_buf_len += 1;
            self.alt_run_chars += 1;
            return;
        }
        // Shift window
        let prev = self.alt_buf[1];
        self.alt_buf[0] = prev;
        self.alt_buf[1] = ch;
        self.alt_run_chars += 1;

        let pair = (self.alt_buf[0], self.alt_buf[1]);
        if pair.0 == pair.1 {
            // Same char twice — not an alternating pattern. Clear alt state.
            self.alt_pair = None;
            self.alt_count = 0;
            return;
        }
        if Some(pair) == self.alt_pair {
            self.alt_count += 1;
        } else if let Some(ref current) = self.alt_pair {
            // Check if this is the "other" direction of the cycle
            // For "-_": pairs are ('-','_') then ('_','-')
            if pair.0 == current.1 && pair.1 == current.0 {
                // This is the return stroke of the cycle — count it
            } else {
                // Different pattern — start fresh
                self.alt_pair = Some(pair);
                self.alt_count = 1;
                self.alt_run_chars = 2;
            }
        } else {
            self.alt_pair = Some(pair);
            self.alt_count = 1;
        }
    }

    fn check_warnings(&mut self) -> Option<RepetitionWarning> {
        // Character run check
        if self.run_length >= MAX_CHAR_RUN {
            let ch = self.last_char.unwrap_or('?');
            return Some(RepetitionWarning {
                kind: RepetitionKind::CharRun {
                    ch,
                    count: self.run_length,
                },
                message: format!(
                    "Repetitive character \"{ch}\" detected ({}+ times in a row). Model output may be degenerate.",
                    self.run_length
                ),
            });
        }

        // Alternating pattern check: 2 chars * MAX_ALT_CYCLES = chars needed
        if self.alt_count >= MAX_ALT_CYCLES {
            if let Some((a, b)) = self.alt_pair {
                return Some(RepetitionWarning {
                    kind: RepetitionKind::AlternatingPattern {
                        pattern: format!("{a}{b}"),
                        cycles: self.alt_count,
                    },
                    message: format!(
                        "Alternating pattern \"{a}{b}\" repeated {}+ times. Model output may be degenerate.",
                        self.alt_count
                    ),
                });
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_character_run() {
        let mut d = RepetitionDetector::default();
        let mut warning = None;
        for _ in 0..79 {
            warning = d.feed_text("a");
        }
        assert!(warning.is_none());
        warning = d.feed_text("a");
        assert!(warning.is_some());
        assert!(matches!(
            warning.unwrap().kind,
            RepetitionKind::CharRun { .. }
        ));
    }

    #[test]
    fn resets_char_run_on_different_char() {
        let mut d = RepetitionDetector::default();
        for _ in 0..50 {
            assert!(d.feed_text("a").is_none());
        }
        assert!(d.run_length == 50);
        assert!(d.feed_text("b").is_none());
        assert!(d.run_length == 1);
        assert!(d.last_char == Some('b'));
    }

    #[test]
    fn detects_alternating_pattern() {
        let mut d = RepetitionDetector::default();
        // 30 cycles: alt_count reaches 29 (off-by-1 from buffer fill and
        // initial pair establishment).
        for _ in 0..30 {
            d.feed_text("-");
            d.feed_text("_");
        }
        // 31st cycle first char: alt_count hits 30 → triggers warning.
        let w = d.feed_text("-");
        assert!(w.is_some());
        assert!(matches!(
            w.unwrap().kind,
            RepetitionKind::AlternatingPattern { .. }
        ));
    }

    #[test]
    fn feeds_multibyte_text() {
        let mut d = RepetitionDetector::default();
        // 80 consecutive 'ç' (each is 2 bytes in UTF-8)
        let s = "ç".repeat(80);
        let w = d.feed_text(&s);
        assert!(w.is_some());
        assert!(matches!(
            w.unwrap().kind,
            RepetitionKind::CharRun { ch: 'ç', .. }
        ));
    }
}
