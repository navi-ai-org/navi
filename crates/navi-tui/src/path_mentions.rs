//! `@` path mentions — pick a project file/folder into the composer (Grok/Cursor style).
//!
//! Typing `@` opens a filtered path palette. Selecting an entry inserts `@rel/path`
//! (directories get a trailing `/`). On submit, mentions are hydrated into the
//! model-facing text so the agent sees file contents / directory listings.

use std::fs;
use std::path::Path;

use crate::TuiApp;
use crate::input::{delete_input_previous_char, insert_input_char};
use crate::notifications::show_notification;
use crate::state::ModalKind;
use crossterm::event::{KeyCode, KeyModifiers};

const MAX_LIST: usize = 200;
const MAX_FILE_HYDRATE_BYTES: u64 = 256 * 1024;
const MAX_DIR_LIST_ENTRIES: usize = 80;
const VISIBLE_ROWS: usize = 12;

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".navi",
    ".grok",
    "dist",
    "build",
    ".next",
    "__pycache__",
    ".venv",
    "venv",
    ".cargo",
];

#[derive(Debug, Clone)]
pub(crate) struct PathCandidate {
    /// Path relative to project root, using `/` separators. Directories end with `/`.
    pub rel: String,
    pub is_dir: bool,
}

/// Open the `@` path palette. `mention_start` is the byte offset of the `@` in `app.input`.
pub(crate) fn open_path_mentions(app: &mut TuiApp, mention_start: usize) {
    app.path_mention_start = Some(mention_start);
    app.path_filter.clear();
    app.selected_path = 0;
    app.path_scroll = 0;
    crate::keybindings::replace_modal(app, ModalKind::PathMentions);
}

/// Detect an in-progress `@query` ending at the cursor; used to open/update the palette.
pub(crate) fn active_mention_query(input: &str, cursor: usize) -> Option<(usize, String)> {
    let cursor = cursor.min(input.len());
    // Walk back from cursor for a bare `@token` (no whitespace).
    let before = &input[..cursor];
    let at = before.rfind('@')?;
    // `@` must start a token (start of input or after whitespace / punctuation openers).
    if at > 0 {
        let prev = before[..at].chars().last().unwrap_or(' ');
        if prev.is_alphanumeric() || prev == '_' || prev == '/' || prev == '.' || prev == '-' {
            // e.g. email@x — not a path mention
            if prev.is_alphanumeric() {
                return None;
            }
        }
    }
    let query = &before[at + 1..];
    if query.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    // Disallow absolute/parent escape in the live query token.
    if query.starts_with('/') || query.contains("..") {
        return None;
    }
    Some((at, query.to_string()))
}

pub(crate) fn filtered_path_candidates(app: &TuiApp) -> Vec<PathCandidate> {
    let filter = app
        .path_filter
        .trim()
        .trim_start_matches('@')
        .replace('\\', "/");
    list_candidates(&app.project_dir, &filter, MAX_LIST)
}

pub(crate) fn handle_path_mentions_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let candidates = filtered_path_candidates(app);
    let len = candidates.len();
    match code {
        KeyCode::Esc => {
            // Leave the partial `@query` in the input; just close the palette.
            app.path_mention_start = None;
            crate::keybindings::close_active_modal(app);
        }
        KeyCode::Down | KeyCode::Tab => {
            if len > 0 {
                app.selected_path = (app.selected_path + 1).min(len - 1);
                app.path_scroll = scroll_for(app.selected_path, VISIBLE_ROWS);
            }
        }
        KeyCode::Up => {
            app.selected_path = app.selected_path.saturating_sub(1);
            app.path_scroll = scroll_for(app.selected_path, VISIBLE_ROWS);
        }
        KeyCode::PageDown => {
            if len > 0 {
                app.selected_path = (app.selected_path + 8).min(len - 1);
                app.path_scroll = scroll_for(app.selected_path, VISIBLE_ROWS);
            }
        }
        KeyCode::PageUp => {
            app.selected_path = app.selected_path.saturating_sub(8);
            app.path_scroll = scroll_for(app.selected_path, VISIBLE_ROWS);
        }
        KeyCode::Enter => {
            if let Some(choice) = candidates.get(app.selected_path).cloned() {
                apply_path_selection(app, &choice);
            } else {
                app.path_mention_start = None;
                crate::keybindings::close_active_modal(app);
            }
        }
        KeyCode::Backspace if modifiers.is_empty() => {
            // Edit the query in the composer: delete char before cursor.
            if app.input_cursor > 0 {
                delete_input_previous_char(app);
            }
            sync_filter_from_input(app);
            // Closed if the `@` was deleted.
            if active_mention_query(&app.input, app.input_cursor).is_none() {
                app.path_mention_start = None;
                crate::keybindings::close_active_modal(app);
            }
        }
        KeyCode::Char(ch) if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
            insert_input_char(app, ch);
            sync_filter_from_input(app);
        }
        _ => {}
    }
    false
}

fn sync_filter_from_input(app: &mut TuiApp) {
    if let Some((at, query)) = active_mention_query(&app.input, app.input_cursor) {
        app.path_mention_start = Some(at);
        app.path_filter = query;
        app.selected_path = 0;
        app.path_scroll = 0;
    } else {
        app.path_filter.clear();
    }
}

fn apply_path_selection(app: &mut TuiApp, choice: &PathCandidate) {
    let Some(start) = app.path_mention_start.or_else(|| {
        active_mention_query(&app.input, app.input_cursor).map(|(at, _)| at)
    }) else {
        crate::keybindings::close_active_modal(app);
        return;
    };
    let cursor = app.input_cursor.min(app.input.len());
    let start = start.min(cursor);
    // Replace `@query` with `@rel/path` (+ trailing space for continued typing).
    let mention = format!("@{} ", choice.rel);
    app.input.replace_range(start..cursor, &mention);
    app.input_cursor = start + mention.len();
    app.input_selection = None;
    app.path_mention_start = None;
    app.path_filter.clear();
    crate::keybindings::close_all_modals(app);
    show_notification(
        app,
        "Mention",
        format!(
            "{} {}",
            if choice.is_dir { "Folder" } else { "File" },
            choice.rel
        ),
    );
}

fn scroll_for(selected: usize, visible: usize) -> usize {
    if selected < visible {
        0
    } else {
        selected + 1 - visible
    }
}

fn list_candidates(project_dir: &Path, filter: &str, limit: usize) -> Vec<PathCandidate> {
    let filter_lower = filter.to_lowercase();
    let mut out = Vec::new();

    // If filter contains a path prefix (dir/), list under that directory first.
    let (base_rel, name_filter) = match filter_lower.rfind('/') {
        Some(i) => (&filter[..i + 1], &filter_lower[i + 1..]),
        None => ("", filter_lower.as_str()),
    };

    let search_root = if base_rel.is_empty() {
        project_dir.to_path_buf()
    } else {
        project_dir.join(base_rel.trim_end_matches('/'))
    };

    if search_root.is_dir() {
        collect_dir(
            project_dir,
            &search_root,
            base_rel,
            name_filter,
            &mut out,
            limit,
            0,
        );
    }

    // Also fuzzy-walk from project root when filter has no slash or few hits.
    if out.len() < 20 && !filter.is_empty() && !filter.contains('/') {
        walk_fuzzy(project_dir, project_dir, "", &filter_lower, &mut out, limit, 0);
    }

    // Prefer shorter paths, dirs first for same depth, then name match.
    out.sort_by(|a, b| {
        let a_score = score(&a.rel, filter);
        let b_score = score(&b.rel, filter);
        a_score
            .cmp(&b_score)
            .then_with(|| b.is_dir.cmp(&a.is_dir))
            .then_with(|| a.rel.cmp(&b.rel))
    });
    out.dedup_by(|a, b| a.rel == b.rel);
    out.truncate(limit);
    out
}

fn score(rel: &str, filter: &str) -> (i32, usize) {
    if filter.is_empty() {
        return (0, rel.len());
    }
    let lower = rel.to_lowercase();
    let f = filter.to_lowercase();
    let name = Path::new(rel)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(rel)
        .to_lowercase();
    if name.starts_with(&f) {
        return (-100, rel.len());
    }
    if name.contains(&f) {
        return (-50, rel.len());
    }
    if lower.contains(&f) {
        return (-10, rel.len());
    }
    // subsequence
    if is_subsequence(&name, &f) {
        return (10, rel.len());
    }
    (100, rel.len())
}

fn is_subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    for ch in needle.chars() {
        loop {
            match it.next() {
                Some(c) if c == ch => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

fn collect_dir(
    project_dir: &Path,
    dir: &Path,
    base_rel: &str,
    name_filter: &str,
    out: &mut Vec<PathCandidate>,
    limit: usize,
    depth: usize,
) {
    if out.len() >= limit || depth > 6 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') && name != ".env" && name != ".gitignore" {
            // keep common dotfiles; skip most hidden
            if !name_filter.is_empty() && !name.to_lowercase().contains(name_filter) {
                continue;
            }
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && SKIP_DIRS.iter().any(|s| *s == name) {
            continue;
        }
        if !name_filter.is_empty() && !name.to_lowercase().contains(name_filter) {
            // still allow dirs that might contain matches when filter empty-ish
            if !is_dir {
                continue;
            }
        }
        let rel = format!("{base_rel}{name}{}", if is_dir { "/" } else { "" });
        // Ensure path is under project
        if let Ok(abs) = entry.path().canonicalize() {
            if !abs.starts_with(project_dir) && !entry.path().starts_with(project_dir) {
                continue;
            }
        }
        out.push(PathCandidate { rel, is_dir });
    }
}

fn walk_fuzzy(
    project_dir: &Path,
    dir: &Path,
    rel_prefix: &str,
    filter: &str,
    out: &mut Vec<PathCandidate>,
    limit: usize,
    depth: usize,
) {
    if out.len() >= limit || depth > 5 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        if out.len() >= limit {
            break;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && SKIP_DIRS.iter().any(|s| *s == name) {
            continue;
        }
        let rel = if rel_prefix.is_empty() {
            format!("{name}{}", if is_dir { "/" } else { "" })
        } else {
            format!("{rel_prefix}{name}{}", if is_dir { "/" } else { "" })
        };
        let lower = rel.to_lowercase();
        if lower.contains(filter) || is_subsequence(&name.to_lowercase(), filter) {
            out.push(PathCandidate {
                rel: rel.clone(),
                is_dir,
            });
        }
        if is_dir {
            walk_fuzzy(
                project_dir,
                &entry.path(),
                &rel,
                filter,
                out,
                limit,
                depth + 1,
            );
        }
    }
}

/// Expand `@path` tokens in a user prompt into path + content for the model.
pub(crate) fn hydrate_path_mentions(project_dir: &Path, text: &str) -> String {
    let mentions = extract_mentions(text);
    if mentions.is_empty() {
        return text.to_string();
    }

    let mut out = text.to_string();
    out.push_str("\n\n---\nAttached paths (@ mentions):\n");
    for rel in mentions {
        let clean = rel.trim_start_matches('@');
        let path = project_dir.join(clean.trim_end_matches('/'));
        if path.is_file() {
            match read_file_capped(&path, MAX_FILE_HYDRATE_BYTES) {
                Ok(content) => {
                    out.push_str(&format!("\n<file path=\"{clean}\">\n{content}\n</file>\n"));
                }
                Err(err) => {
                    out.push_str(&format!("\n<file path=\"{clean}\">\n(error: {err})\n</file>\n"));
                }
            }
        } else if path.is_dir() {
            let listing = list_dir_brief(&path, MAX_DIR_LIST_ENTRIES);
            out.push_str(&format!(
                "\n<directory path=\"{clean}\">\n{listing}\n</directory>\n"
            ));
        } else {
            out.push_str(&format!(
                "\n<path path=\"{clean}\">(not found under project)</path>\n"
            ));
        }
    }
    out
}

fn extract_mentions(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find('@') {
        let after = &rest[i + 1..];
        // token until whitespace
        let end = after
            .find(|c: char| c.is_whitespace() || c == '`' || c == '"' || c == '\'' || c == ')')
            .unwrap_or(after.len());
        let token = after[..end].trim();
        if !token.is_empty()
            && !token.starts_with('/')
            && !token.contains("..")
            && token.chars().all(|c| {
                c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '~')
            })
        {
            let full = format!("@{token}");
            if !found.iter().any(|f: &String| f == &full) {
                found.push(full);
            }
        }
        rest = &after[end.min(after.len())..];
        if rest.is_empty() {
            break;
        }
    }
    found
}

fn read_file_capped(path: &Path, max_bytes: u64) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > max_bytes {
        let data = fs::read(path).map_err(|e| e.to_string())?;
        let take = max_bytes as usize;
        let slice = &data[..take.min(data.len())];
        let mut s = String::from_utf8_lossy(slice).into_owned();
        s.push_str(&format!(
            "\n… truncated ({} bytes total, showing first {max_bytes})",
            meta.len()
        ));
        return Ok(s);
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}

fn list_dir_brief(path: &Path, limit: usize) -> String {
    let Ok(rd) = fs::read_dir(path) else {
        return "(unreadable)".into();
    };
    let mut lines = Vec::new();
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries.into_iter().take(limit) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        lines.push(if is_dir {
            format!("{name}/")
        } else {
            name
        });
    }
    if lines.is_empty() {
        "(empty)".into()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn active_mention_query_basic() {
        let text = "look at @src/fo";
        let (at, q) = active_mention_query(text, text.len()).unwrap();
        assert_eq!(at, 8);
        assert_eq!(q, "src/fo");
    }

    #[test]
    fn active_mention_ignores_email() {
        assert!(active_mention_query("mail me@x.com", 13).is_none());
    }

    #[test]
    fn hydrate_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.rs");
        fs::write(&file, "fn main() {}").unwrap();
        let out = hydrate_path_mentions(dir.path(), "see @hello.rs please");
        assert!(out.contains("<file path=\"hello.rs\">"));
        assert!(out.contains("fn main() {}"));
    }

    #[test]
    fn list_candidates_finds_nested() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/lib.rs"), "//").unwrap();
        let hits = list_candidates(dir.path(), "lib", 50);
        assert!(hits.iter().any(|c| c.rel.contains("lib.rs")), "{hits:?}");
    }
}
