//! scrollback block selection helpers.
//!
//! Each discrete chat entry (user message, assistant message, tool result,
//! tool group, subagent) is a selectable block. Selection and text-drag stay
//! inside one block instead of bleeding across entries.

use crate::TuiApp;
use crate::mouse::copy_text_to_clipboard;
use crate::render::tool::{tool_compact_text, tool_full_content};
use crate::state::{ChatLineSource, ChatRole};

/// Ordered unique blocks from cached line sources (skipping gaps).
pub(crate) fn chat_blocks(app: &TuiApp) -> Vec<ChatLineSource> {
    let cache = app.chat_render_cache.borrow();
    let mut blocks = Vec::new();
    for source in &cache.line_sources {
        if matches!(source, ChatLineSource::None) {
            continue;
        }
        if blocks
            .last()
            .is_some_and(|last| chat_sources_match(last, source))
        {
            continue;
        }
        blocks.push(source.clone());
    }
    blocks
}

pub(crate) fn chat_sources_match(a: &ChatLineSource, b: &ChatLineSource) -> bool {
    match (a, b) {
        (ChatLineSource::Message(left), ChatLineSource::Message(right)) => left == right,
        (ChatLineSource::ToolResult(left), ChatLineSource::ToolResult(right)) => left == right,
        (ChatLineSource::ToolGroup(left), ChatLineSource::ToolGroup(right)) => left == right,
        (ChatLineSource::Subagent(left), ChatLineSource::Subagent(right)) => left == right,
        (ChatLineSource::None, ChatLineSource::None) => true,
        _ => false,
    }
}

pub(crate) fn source_at_line(app: &TuiApp, line_index: usize) -> Option<ChatLineSource> {
    let cache = app.chat_render_cache.borrow();
    cache
        .line_sources
        .get(line_index)
        .filter(|s| !matches!(s, ChatLineSource::None))
        .cloned()
}

/// Inclusive line range for a block inside the current render cache.
pub(crate) fn block_line_range(app: &TuiApp, source: &ChatLineSource) -> Option<(usize, usize)> {
    let cache = app.chat_render_cache.borrow();
    let mut start = None;
    let mut end = None;
    for (idx, line_source) in cache.line_sources.iter().enumerate() {
        if chat_sources_match(line_source, source) {
            if start.is_none() {
                start = Some(idx);
            }
            end = Some(idx);
        } else if start.is_some() {
            // Contiguous block finished.
            break;
        }
    }
    match (start, end) {
        (Some(s), Some(e)) => Some((s, e)),
        _ => None,
    }
}

/// Clamp a line index to the contiguous range of `bound` (if any).
pub(crate) fn clamp_line_to_block(
    app: &TuiApp,
    line_index: usize,
    bound: &Option<ChatLineSource>,
) -> usize {
    let Some(source) = bound else {
        return line_index;
    };
    let Some((start, end)) = block_line_range(app, source) else {
        return line_index;
    };
    line_index.clamp(start, end)
}

pub(crate) fn select_chat_block(app: &mut TuiApp, source: ChatLineSource) {
    if matches!(source, ChatLineSource::None) {
        return;
    }
    app.selected_chat_source = Some(source);
    // Keep text drag selection cleared when jumping blocks.
    if app.selection.as_ref().is_some_and(|s| !s.active) {
        app.selection = None;
    }
    ensure_selected_block_visible(app);
}

pub(crate) fn clear_selected_block(app: &mut TuiApp) {
    app.selected_chat_source = None;
}

pub(crate) fn select_adjacent_block(app: &mut TuiApp, delta: isize) {
    let blocks = chat_blocks(app);
    if blocks.is_empty() {
        return;
    }
    let current = app
        .selected_chat_source
        .as_ref()
        .and_then(|selected| {
            blocks
                .iter()
                .position(|block| chat_sources_match(block, selected))
        })
        .unwrap_or(if delta < 0 {
            blocks.len().saturating_sub(1)
        } else {
            0
        });
    let next = if delta < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1).min(blocks.len().saturating_sub(1))
    };
    if let Some(block) = blocks.get(next).cloned() {
        select_chat_block(app, block);
    }
}

fn ensure_selected_block_visible(app: &mut TuiApp) {
    let Some(source) = app.selected_chat_source.clone() else {
        return;
    };
    let Some((start, end)) = block_line_range(app, &source) else {
        return;
    };
    let cache = app.chat_render_cache.borrow();
    let total = cache.lines.len();
    let visible = cache
        .chat_rect
        .map(|r| r.height as usize)
        .unwrap_or(20)
        .max(1);
    drop(cache);

    let max_scroll = total.saturating_sub(visible);
    // Bottom-anchored scroll: offset 0 shows the last page.
    let first_visible = total
        .saturating_sub(visible)
        .saturating_sub(app.scroll_offset.min(max_scroll));
    let last_visible = first_visible.saturating_add(visible.saturating_sub(1));

    if end < first_visible {
        // Block is above viewport → scroll up (increase offset).
        let desired_first = end.saturating_sub(visible.saturating_sub(1));
        app.scroll_offset = total
            .saturating_sub(visible)
            .saturating_sub(desired_first)
            .min(max_scroll);
    } else if start > last_visible {
        // Block is below viewport → scroll down (decrease offset).
        app.scroll_offset = total
            .saturating_sub(visible)
            .saturating_sub(start)
            .min(max_scroll);
    }
}

/// Plain text of the selected block for clipboard copy.
pub(crate) fn selected_block_text(app: &TuiApp) -> Option<String> {
    let source = app.selected_chat_source.as_ref()?;
    match source {
        ChatLineSource::Message(index) => {
            let msg = app.messages.get(*index)?;
            let mut parts = Vec::new();
            if !msg.thinking_content.trim().is_empty() && app.show_thinking {
                parts.push(msg.thinking_content.clone());
            }
            if !msg.content.trim().is_empty() {
                parts.push(msg.content.clone());
            }
            if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result) {
                if app.full_tool_view {
                    parts.push(tool_full_content(inv, result));
                } else {
                    parts.push(tool_compact_text(inv, result));
                }
            }
            let text = parts.join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        ChatLineSource::ToolResult(id) => {
            for msg in &app.messages {
                if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result)
                    && result.invocation_id == *id
                {
                    let show = crate::render::tool_policy::tool_body_visible(
                        inv,
                        result,
                        app.full_tool_view,
                        &app.expanded_tool_results,
                        &app.collapsed_tool_results,
                    );
                    return Some(if show {
                        tool_full_content(inv, result)
                    } else {
                        tool_compact_text(inv, result)
                    });
                }
            }
            None
        }
        ChatLineSource::ToolGroup(ids) => {
            let mut chunks = Vec::new();
            for id in ids {
                for msg in &app.messages {
                    if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result)
                        && result.invocation_id == *id
                    {
                        chunks.push(tool_compact_text(inv, result));
                    }
                }
            }
            (!chunks.is_empty()).then_some(chunks.join("\n"))
        }
        ChatLineSource::Subagent(id) => app
            .subagent_transcripts
            .get(id)
            .map(|t| {
                let mut lines = vec![t.title.clone()];
                for item in &t.items {
                    lines.push(item.title.clone());
                    if let Some(detail) = &item.detail {
                        lines.push(format!("  {detail}"));
                    }
                }
                lines.join("\n")
            })
            .filter(|s| !s.is_empty()),
        ChatLineSource::None => None,
    }
}

pub(crate) fn copy_selected_block(app: &mut TuiApp) -> bool {
    if let Some(text) = selected_block_text(app) {
        copy_text_to_clipboard(app, &text);
        true
    } else {
        false
    }
}

/// Activate the selected block (open actions / expand tool / open subagent).
pub(crate) fn activate_selected_block(app: &mut TuiApp) {
    let Some(source) = app.selected_chat_source.clone() else {
        return;
    };
    match source {
        ChatLineSource::Message(index) => {
            if app
                .messages
                .get(index)
                .is_some_and(|m| m.role == ChatRole::User)
            {
                app.message_action_target = Some(index);
                app.selected_message_action = 0;
                crate::keybindings::replace_modal(app, crate::state::ModalKind::MessageActions);
            }
            // Assistant messages: selection alone is enough; content already visible.
        }
        ChatLineSource::ToolResult(id) => {
            for msg in &app.messages {
                if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result)
                    && result.invocation_id == id
                {
                    crate::render::tool_policy::toggle_tool_body(
                        inv,
                        result,
                        app.full_tool_view,
                        &mut app.expanded_tool_results,
                        &mut app.collapsed_tool_results,
                    );
                    app.chat_render_cache.borrow_mut().signature_hash = 0;
                    return;
                }
            }
        }
        ChatLineSource::ToolGroup(ids) => {
            for id in ids {
                for msg in &app.messages {
                    if let (Some(inv), Some(result)) = (&msg.tool_invocation, &msg.tool_result)
                        && result.invocation_id == id
                    {
                        crate::render::tool_policy::toggle_tool_body(
                            inv,
                            result,
                            app.full_tool_view,
                            &mut app.expanded_tool_results,
                            &mut app.collapsed_tool_results,
                        );
                        break;
                    }
                }
            }
            app.chat_render_cache.borrow_mut().signature_hash = 0;
        }
        ChatLineSource::Subagent(id) => {
            app.open_subagent_view(id);
        }
        ChatLineSource::None => {}
    }
}
