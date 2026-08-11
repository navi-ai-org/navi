//! Status diamonds for tool/activity indicators.
//!
//! filled diamond (`◆`) without the left quote-bar / vertical trail.
//! Running states pulse between filled and hollow so the user can see activity.

use std::time::Instant;

use ratatui::style::Color;
use unicode_width::UnicodeWidthStr;

/// Filled diamond used for settled status (success / error).
pub(crate) const DIAMOND: &str = "◆";
/// Hollow diamond used as the alternate frame of the running pulse.
pub(crate) const DIAMOND_HOLLOW: &str = "◇";

/// Frame duration for the running pulse (filled ↔ hollow).
/// Keep in sync with event-loop redraw cadence while tools run (~60fps poll,
/// this is the visual half-period).
pub(crate) const RUNNING_FRAME_MS: u64 = 320;

/// Glyph for a settled tool/result status. Always the filled diamond — color
/// carries success vs error.
pub(crate) fn settled_diamond() -> &'static str {
    DIAMOND
}

/// Prefix (glyph + trailing space) for a settled tool line.
pub(crate) fn settled_diamond_prefix(ok: bool) -> &'static str {
    // Same glyph either way; callers color it green/red.
    let _ = ok;
    "◆ "
}

/// Animated diamond for in-flight work. Pulses filled ↔ hollow.
///
/// No vertical bar / corner stroke — only the diamond itself.
///
/// Frame sequence (period 4 × [`RUNNING_FRAME_MS`]):
/// `◆  ◇  ◆  ◇` — a steady heartbeat while the tool has no result yet.
pub(crate) fn running_diamond(elapsed_ms: u64) -> &'static str {
    match (elapsed_ms / RUNNING_FRAME_MS) % 4 {
        0 | 2 => DIAMOND,
        _ => DIAMOND_HOLLOW,
    }
}

/// Running diamond with trailing space for list rows.
pub(crate) fn running_diamond_prefix(elapsed_ms: u64) -> &'static str {
    match (elapsed_ms / RUNNING_FRAME_MS) % 4 {
        0 | 2 => "◆ ",
        _ => "◇ ",
    }
}

/// Discrete pulse frame index (for cache invalidation).
pub(crate) fn running_pulse_frame(elapsed_ms: u64) -> u64 {
    elapsed_ms / RUNNING_FRAME_MS
}

/// Color for a settled diamond.
pub(crate) fn settled_diamond_color(ok: bool, success: Color, error: Color) -> Color {
    if ok { success } else { error }
}

/// Color for the in-flight diamond — warm signal so it reads as “active”.
pub(crate) fn running_diamond_color(accent: Color) -> Color {
    accent
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityAnimation {
    Idle,
    Working,
    Error,
    Success,
}

impl ActivityAnimation {
    pub(crate) const fn is_transient(self) -> bool {
        matches!(self, Self::Error | Self::Success)
    }

    pub(crate) const fn loops(self) -> bool {
        matches!(self, Self::Idle | Self::Working)
    }

    pub(crate) const fn duration_ms(self) -> u64 {
        match self {
            Self::Idle => 2_200,
            Self::Working => 1_350,
            Self::Error => 1_800,
            Self::Success => 4_000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActivityAnimationState {
    pub(crate) animation: ActivityAnimation,
    pub(crate) started_at: Instant,
}

impl ActivityAnimationState {
    pub(crate) fn new(animation: ActivityAnimation) -> Self {
        Self {
            animation,
            started_at: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ActivityFrame {
    text: &'static str,
    duration_ms: u64,
}

const IDLE_FRAMES: [ActivityFrame; 3] = [
    ActivityFrame {
        text: "(￣ω￣) z",
        duration_ms: 650,
    },
    ActivityFrame {
        text: "(－ω－) zz",
        duration_ms: 650,
    },
    ActivityFrame {
        text: "(￣ω￣) zzZ",
        duration_ms: 900,
    },
];

/// Unified writing animation for any working state (thinking, streaming, tools,
/// approvals, background commands). One face per state was not a real
/// animation and the per-state variants read as noise — a single expressive
/// writing loop reads as "the agent is busy" without lying about phase.
///
/// The pencil (φ) slides left→right across the three frames so the whole
/// glyph animates, not just the eyes. Frames stay at display width 9 so the
/// status label + elapsed time still fit on narrow terminals.
const WORKING_FRAMES: [ActivityFrame; 3] = [
    ActivityFrame {
        text: "φ__(．．)",
        duration_ms: 450,
    },
    ActivityFrame {
        text: "_φ_(．．)",
        duration_ms: 450,
    },
    ActivityFrame {
        text: "__φ(．．)",
        duration_ms: 450,
    },
];

const ERROR_FRAMES: [ActivityFrame; 3] = [
    ActivityFrame {
        text: "(・_・;)",
        duration_ms: 400,
    },
    ActivityFrame {
        text: "(╬ Ò﹏Ó)",
        duration_ms: 600,
    },
    ActivityFrame {
        text: "(╯°□°）╯︵ ┻━┻",
        duration_ms: 800,
    },
];

const SUCCESS_FRAMES: [ActivityFrame; 3] = [
    ActivityFrame {
        text: "(・_・;)",
        duration_ms: 400,
    },
    ActivityFrame {
        text: "(ﾉ≧∀≦)ﾉ★",
        duration_ms: 600,
    },
    ActivityFrame {
        text: "＼(≧▽≦)／",
        duration_ms: 3_000,
    },
];

fn activity_frames(animation: ActivityAnimation) -> &'static [ActivityFrame] {
    match animation {
        ActivityAnimation::Idle => &IDLE_FRAMES,
        ActivityAnimation::Working => &WORKING_FRAMES,
        ActivityAnimation::Error => &ERROR_FRAMES,
        ActivityAnimation::Success => &SUCCESS_FRAMES,
    }
}

pub(crate) fn activity_frame(animation: ActivityAnimation, elapsed_ms: u64) -> &'static str {
    let frames = activity_frames(animation);
    let cycle_position = if animation.loops() {
        elapsed_ms % animation.duration_ms()
    } else {
        elapsed_ms.min(animation.duration_ms().saturating_sub(1))
    };
    let mut remaining = cycle_position;

    for frame in frames {
        if remaining < frame.duration_ms {
            return frame.text;
        }
        remaining -= frame.duration_ms;
    }

    frames.last().map(|frame| frame.text).unwrap_or("")
}

pub(crate) fn activity_frame_width(animation: ActivityAnimation) -> usize {
    activity_frames(animation)
        .iter()
        .map(|frame| UnicodeWidthStr::width(frame.text))
        .max()
        .unwrap_or(0)
}

pub(crate) fn padded_activity_frame(animation: ActivityAnimation, elapsed_ms: u64) -> String {
    let frame = activity_frame(animation, elapsed_ms);
    let padding = activity_frame_width(animation).saturating_sub(UnicodeWidthStr::width(frame));
    format!("{frame}{}", " ".repeat(padding))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_diamond_pulses_between_filled_and_hollow() {
        assert_eq!(running_diamond(0), DIAMOND);
        assert_eq!(running_diamond(RUNNING_FRAME_MS), DIAMOND_HOLLOW);
        assert_eq!(running_diamond(RUNNING_FRAME_MS * 2), DIAMOND);
        assert_eq!(running_diamond(RUNNING_FRAME_MS * 3), DIAMOND_HOLLOW);
    }

    #[test]
    fn settled_prefix_is_diamond_only_no_bar() {
        let prefix = settled_diamond_prefix(true);
        assert!(prefix.contains(DIAMOND));
        assert!(!prefix.contains('│'));
        assert!(!prefix.contains('|'));
        assert!(!prefix.contains('┃'));
    }

    #[test]
    fn activity_animations_use_compact_transitions() {
        assert_eq!(activity_frame(ActivityAnimation::Idle, 0), "(￣ω￣) z");
        assert_eq!(activity_frame(ActivityAnimation::Idle, 650), "(－ω－) zz");
        assert_eq!(
            activity_frame(ActivityAnimation::Idle, 1_300),
            "(￣ω￣) zzZ"
        );
        assert_eq!(activity_frame(ActivityAnimation::Working, 0), "φ__(．．)");
        assert_eq!(activity_frame(ActivityAnimation::Working, 450), "_φ_(．．)");
        assert_eq!(activity_frame(ActivityAnimation::Working, 900), "__φ(．．)");
        assert_eq!(
            activity_frame(ActivityAnimation::Error, 1_000),
            "(╯°□°）╯︵ ┻━┻"
        );
        assert_eq!(
            activity_frame(ActivityAnimation::Success, 1_000),
            "＼(≧▽≦)／"
        );
    }

    #[test]
    fn looping_animations_wrap_and_terminal_animations_hold_final_frame() {
        assert_eq!(
            activity_frame(
                ActivityAnimation::Idle,
                ActivityAnimation::Idle.duration_ms()
            ),
            activity_frame(ActivityAnimation::Idle, 0)
        );
        assert_eq!(
            activity_frame(
                ActivityAnimation::Working,
                ActivityAnimation::Working.duration_ms()
            ),
            activity_frame(ActivityAnimation::Working, 0)
        );
        assert_eq!(
            activity_frame(
                ActivityAnimation::Success,
                ActivityAnimation::Success.duration_ms() + 1_000
            ),
            "＼(≧▽≦)／"
        );
    }

    #[test]
    fn padded_animation_frames_have_stable_display_width() {
        for animation in [
            ActivityAnimation::Idle,
            ActivityAnimation::Working,
            ActivityAnimation::Error,
            ActivityAnimation::Success,
        ] {
            let expected = activity_frame_width(animation);
            for elapsed_ms in (0..animation.duration_ms()).step_by(100) {
                assert_eq!(
                    UnicodeWidthStr::width(padded_activity_frame(animation, elapsed_ms).as_str()),
                    expected,
                    "unstable width for {animation:?} at {elapsed_ms}ms"
                );
            }
        }
    }
}
