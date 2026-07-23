//! Compact large clipboard pastes in the composer as chips.
//!
//! Thresholds (either is enough to compact):
//! - more than 2 newlines (4+ lines of text)
//! - more than 850 characters
//!
//! Tag form in the draft: `[Pasted text #1 +64 lines]`
//! Full body lives in [`crate::app::TuiApp::pending_pastes`] until submit/expand.

use crate::app::TuiApp;
use crate::input::insert_input_text;
use crate::state::PendingPaste;

/// Compact when paste has more than this many newline characters.
pub(crate) const PASTE_COMPACT_NEWLINE_THRESHOLD: usize = 2;
/// Compact when paste is longer than this many Unicode characters.
pub(crate) const PASTE_COMPACT_CHAR_THRESHOLD: usize = 850;

const TAG_PREFIX: &str = "[Pasted text #";

/// Normalize paste body and decide whether to show a chip instead of raw text.
pub(crate) fn should_compact_paste(text: &str) -> bool {
    let normalized = normalize_paste_text(text);
    if normalized.is_empty() {
        return false;
    }
    let newlines = count_newlines(&normalized);
    let chars = normalized.chars().count();
    newlines > PASTE_COMPACT_NEWLINE_THRESHOLD || chars > PASTE_COMPACT_CHAR_THRESHOLD
}

pub(crate) fn normalize_paste_text(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) fn count_newlines(text: &str) -> usize {
    text.chars().filter(|&c| c == '\n').count()
}

/// Visual line count for the chip (`+N lines`): newlines + 1 when non-empty.
pub(crate) fn paste_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        count_newlines(text) + 1
    }
}

pub(crate) fn paste_tag(index_1based: usize, line_count: usize) -> String {
    let unit = if line_count == 1 { "line" } else { "lines" };
    format!("{TAG_PREFIX}{index_1based} +{line_count} {unit}]")
}

/// Insert paste as a compact chip when thresholds are met; otherwise insert raw text.
pub(crate) fn insert_paste_into_composer(app: &mut TuiApp, content: &str) {
    let normalized = normalize_paste_text(content);
    if normalized.is_empty() {
        return;
    }
    if !should_compact_paste(&normalized) {
        insert_input_text(app, &normalized);
        return;
    }

    let line_count = paste_line_count(&normalized);
    app.pending_pastes.push(PendingPaste {
        text: normalized,
        line_count,
    });
    let index = app.pending_pastes.len();
    let tag = paste_tag(index, line_count);
    insert_input_text(app, &tag);
}

/// Expand every paste chip in `text` using `pastes` (1-based index in the tag).
/// Unmatched tags are left as-is.
pub(crate) fn expand_paste_tags(text: &str, pastes: &[PendingPaste]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(TAG_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some((tag_len, index_1based, _)) = parse_paste_tag_at(after) {
            let idx = index_1based.saturating_sub(1);
            if let Some(paste) = pastes.get(idx) {
                out.push_str(&paste.text);
            } else {
                out.push_str(&after[..tag_len]);
            }
            rest = &after[tag_len..];
        } else {
            out.push_str(TAG_PREFIX);
            rest = &after[TAG_PREFIX.len()..];
        }
    }
    out.push_str(rest);
    out
}

/// Expand paste chips in the live composer draft (for submit / queue).
pub(crate) fn expand_composer_pastes(app: &mut TuiApp) {
    if app.pending_pastes.is_empty() {
        return;
    }
    if !app.input.contains(TAG_PREFIX) {
        app.pending_pastes.clear();
        return;
    }
    let expanded = expand_paste_tags(&app.input, &app.pending_pastes);
    app.input = expanded;
    app.input_cursor = floor_char_boundary(&app.input, app.input.len());
    app.pending_pastes.clear();
    app.input_selection = None;
}

/// If the cursor sits on a paste chip, expand that chip in-place and return true.
pub(crate) fn try_expand_paste_at_cursor(app: &mut TuiApp) -> bool {
    let Some((start, end, index_1based)) = paste_tag_covering(app.input_cursor, &app.input) else {
        return false;
    };
    let idx = index_1based.saturating_sub(1);
    if idx >= app.pending_pastes.len() {
        return false;
    }
    let body = app.pending_pastes[idx].text.clone();
    app.input.replace_range(start..end, &body);
    // Drop this paste entry and rewrite remaining tags so indices stay dense.
    app.pending_pastes.remove(idx);
    rewrite_paste_tags_after_removal(app, index_1based);
    app.input_cursor = start + body.len();
    app.input_cursor = floor_char_boundary(&app.input, app.input_cursor);
    app.input_selection = None;
    true
}

/// If backspace would delete into a paste chip, remove the whole chip.
pub(crate) fn try_delete_paste_before_cursor(app: &mut TuiApp) -> bool {
    if app.input_cursor == 0 || app.pending_pastes.is_empty() {
        return false;
    }
    let cursor = app.input_cursor;
    // Prefer a tag that ends at the cursor.
    if let Some((start, end, index_1based)) = paste_tag_ending_at(cursor, &app.input) {
        return delete_paste_tag(app, start, end, index_1based);
    }
    // Or cursor is inside a tag.
    if let Some((start, end, index_1based)) = paste_tag_covering(cursor.saturating_sub(1), &app.input)
    {
        return delete_paste_tag(app, start, end, index_1based);
    }
    false
}

fn delete_paste_tag(app: &mut TuiApp, start: usize, end: usize, index_1based: usize) -> bool {
    let idx = index_1based.saturating_sub(1);
    if idx >= app.pending_pastes.len() {
        return false;
    }
    app.input.replace_range(start..end, "");
    app.pending_pastes.remove(idx);
    rewrite_paste_tags_after_removal(app, index_1based);
    app.input_cursor = start.min(app.input.len());
    app.input_cursor = floor_char_boundary(&app.input, app.input_cursor);
    app.input_selection = None;
    true
}

/// After removing paste `removed_1based`, renumber higher tags down by one.
fn rewrite_paste_tags_after_removal(app: &mut TuiApp, removed_1based: usize) {
    if app.pending_pastes.is_empty() {
        return;
    }
    let mut rebuilt = String::with_capacity(app.input.len());
    let mut rest = app.input.as_str();
    while let Some(start) = rest.find(TAG_PREFIX) {
        rebuilt.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some((tag_len, index_1based, line_count)) = parse_paste_tag_at(after) {
            let new_index = if index_1based > removed_1based {
                index_1based - 1
            } else if index_1based == removed_1based {
                // Tag for removed paste still in buffer (shouldn't happen); drop it.
                rest = &after[tag_len..];
                continue;
            } else {
                index_1based
            };
            // Prefer stored line count when available.
            let lines = app
                .pending_pastes
                .get(new_index.saturating_sub(1))
                .map(|p| p.line_count)
                .unwrap_or(line_count);
            rebuilt.push_str(&paste_tag(new_index, lines));
            rest = &after[tag_len..];
        } else {
            rebuilt.push_str(TAG_PREFIX);
            rest = &after[TAG_PREFIX.len()..];
        }
    }
    rebuilt.push_str(rest);
    app.input = rebuilt;
}

/// Parse a paste tag at the start of `s`. Returns (byte_len, 1-based index, line_count).
fn parse_paste_tag_at(s: &str) -> Option<(usize, usize, usize)> {
    if !s.starts_with(TAG_PREFIX) {
        return None;
    }
    let after_prefix = &s[TAG_PREFIX.len()..];
    let mut digits = 0usize;
    for ch in after_prefix.chars() {
        if ch.is_ascii_digit() {
            digits += 1;
        } else {
            break;
        }
    }
    if digits == 0 {
        return None;
    }
    let index_1based: usize = after_prefix[..digits].parse().ok()?;
    let after_num = &after_prefix[digits..];
    // " +N line]" or " +N lines]"
    let after_plus = after_num.strip_prefix(" +")?;
    let mut line_digits = 0usize;
    for ch in after_plus.chars() {
        if ch.is_ascii_digit() {
            line_digits += 1;
        } else {
            break;
        }
    }
    if line_digits == 0 {
        return None;
    }
    let line_count: usize = after_plus[..line_digits].parse().ok()?;
    let after_lines = &after_plus[line_digits..];
    let after_unit = after_lines
        .strip_prefix(" lines]")
        .or_else(|| after_lines.strip_prefix(" line]"))?;
    let tag_len = s.len() - after_unit.len();
    Some((tag_len, index_1based, line_count))
}

fn paste_tag_covering(byte_pos: usize, input: &str) -> Option<(usize, usize, usize)> {
    let mut search_from = 0usize;
    while let Some(rel) = input[search_from..].find(TAG_PREFIX) {
        let start = search_from + rel;
        if let Some((tag_len, index, _)) = parse_paste_tag_at(&input[start..]) {
            let end = start + tag_len;
            if byte_pos >= start && byte_pos < end {
                return Some((start, end, index));
            }
            search_from = end;
        } else {
            search_from = start + TAG_PREFIX.len();
        }
    }
    None
}

fn paste_tag_ending_at(byte_pos: usize, input: &str) -> Option<(usize, usize, usize)> {
    let mut search_from = 0usize;
    while let Some(rel) = input[search_from..].find(TAG_PREFIX) {
        let start = search_from + rel;
        if let Some((tag_len, index, _)) = parse_paste_tag_at(&input[start..]) {
            let end = start + tag_len;
            if end == byte_pos {
                return Some((start, end, index));
            }
            search_from = end;
        } else {
            search_from = start + TAG_PREFIX.len();
        }
    }
    None
}

fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_spec() {
        // 3 lines → 2 newlines → keep raw
        assert!(!should_compact_paste("a\nb\nc"));
        // 4 lines → 3 newlines → compact
        assert!(should_compact_paste("a\nb\nc\nd"));
        // Long single line
        let long = "x".repeat(851);
        assert!(should_compact_paste(&long));
        assert!(!should_compact_paste(&"x".repeat(850)));
    }

    #[test]
    fn expand_replaces_tags() {
        let pastes = vec![PendingPaste {
            text: "hello\nworld".into(),
            line_count: 2,
        }];
        let tag = paste_tag(1, 2);
        let expanded = expand_paste_tags(&format!("before {tag} after"), &pastes);
        assert_eq!(expanded, "before hello\nworld after");
    }

    #[test]
    fn parse_tag_roundtrip() {
        let tag = paste_tag(3, 64);
        let (len, idx, lines) = parse_paste_tag_at(&tag).expect("parse");
        assert_eq!(len, tag.len());
        assert_eq!(idx, 3);
        assert_eq!(lines, 64);
    }
}
