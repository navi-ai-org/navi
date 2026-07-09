//! Status diamonds for tool/activity indicators.
//!
//! Grok-style filled diamond (`◆`) without the left quote-bar / vertical trail.
//! Running states pulse between filled and hollow so the user can see activity.

use ratatui::style::Color;

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
}
