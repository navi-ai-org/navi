EM = "\u2014"
path = 'crates/navi-tui/src/keybindings/modals.rs'
with open(path, 'r') as f:
    s = f.read()

start_marker = 'pub(crate) fn handle_providers_key('
end_marker = 'pub(crate) fn handle_oauth_key(app: &mut TuiApp, code: KeyCode, modifiers: KeyModifiers) -> bool {'

start = s.index(start_marker)
end = s.index(end_marker, start)
prefix = s[:start]
suffix = s[end:]

new_fn = f'''pub(crate) fn handle_providers_key(
    app: &mut TuiApp,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {{
    use crate::providers::ProviderListRow;
    use navi_sdk::provider_catalog;

    let list_rows = app.filtered_providers();
    let catalog = provider_catalog(&app.loaded_config.config);
    let count = list_rows.len();

    // Helper: get the catalog index of the currently selected row (if any).
    let current_catalog_idx = list_rows
        .get(app.selected_provider_setting)
        .and_then(|row| match row {{
            ProviderListRow::Provider {{ index }} => Some(*index),
            ProviderListRow::Header {{ .. }} => None,
        }});

    // Helper: find the nearest non-header catalog index at or after `start`.
    let first_selectable = |start: usize| -> Option<usize> {{
        list_rows
            .iter()
            .skip(start)
            .find_map(|row| match row {{
                ProviderListRow::Provider {{ index }} => Some(*index),
                ProviderListRow::Header {{ .. }} => None,
            }})
    }};

    // Helper: find the nearest non-header catalog index strictly before `start`.
    let last_selectable_before = |start: usize| -> Option<usize> {{
        list_rows
            .iter()
            .take(start)
            .rev()
            .find_map(|row| match row {{
                ProviderListRow::Provider {{ index }} => Some(*index),
                ProviderListRow::Header {{ .. }} => None,
            }})
    }};

    // Helper: convert a catalog index back to a list row position.
    let row_pos_of = |catalog_idx: usize| -> Option<usize> {{
        list_rows.iter().position(|row| matches!(row, ProviderListRow::Provider {{ index }} if *index == catalog_idx))
    }};

    // Selected row position in the visible list (the row whose underlying
    // catalog provider matches the current selection, or fallback to the
    // current numeric position so the highlight stays in place when only
    // headers change).
    let current_row_pos = current_catalog_idx
        .and_then(row_pos_of)
        .unwrap_or(app.selected_provider_setting.min(count.saturating_sub(1)));

    // Helper that returns the provider at a list row position.
    let provider_at = |pos: usize| -> Option<&navi_sdk::ProviderConfig> {{
        match list_rows.get(pos)? {{
            ProviderListRow::Provider {{ index }} => catalog.get(*index),
            ProviderListRow::Header {{ .. }} => None,
        }}
    }};

    let mut new_row_pos = current_row_pos;
    let mut reset_to_first = false;
    let mut reset_filter = false;

    match code {{
        KeyCode::Esc => {{
            app.provider_filter.clear();
            super::close_active_modal(app);
        }}
        KeyCode::Down => {{
            // Move to the next Provider row; if at the end, stay.
            if let Some(next) = first_selectable(current_row_pos + 1) {{
                new_row_pos = row_pos_of(next).unwrap_or(current_row_pos);
            }} else {{
                new_row_pos = current_row_pos;
            }}
        }}
        KeyCode::Up => {{
            if let Some(prev) = last_selectable_before(current_row_pos) {{
                new_row_pos = row_pos_of(prev).unwrap_or(current_row_pos);
            }} else {{
                new_row_pos = current_row_pos;
            }}
        }}
        KeyCode::Enter | KeyCode::Char('k')
            if code == KeyCode::Enter
                || modifiers.contains(KeyModifiers::CONTROL) =>
        {{
            if let Some(provider) = provider_at(current_row_pos).cloned() {{
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }}
        }}
        KeyCode::Char('o') | KeyCode::Char('O') if modifiers.contains(KeyModifiers::CONTROL) => {{
            if let Some(provider) = provider_at(current_row_pos).cloned() {{
                start_provider_oauth(app, &provider);
            }}
        }}
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {{
            if let Some(provider) = provider_at(current_row_pos).cloned() {{
                super::provider_sync::sync_provider_tui(app, &provider.id);
            }}
        }}
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {{
            if let Some(provider) = provider_at(current_row_pos).cloned() {{
                let _ = app.credential_store().delete_api_key(&provider.id);
            }}
        }}
        KeyCode::Char(ch) => {{
            app.provider_filter.push(ch);
            reset_to_first = true;
        }}
        KeyCode::Backspace => {{
            app.provider_filter.pop();
            reset_to_first = true;
        }}
        _ => {{}}
    }}

    if reset_to_first {{
        // After filter change, jump to the first non-header row.
        new_row_pos = first_selectable(0).and_then(row_pos_of).unwrap_or(0);
        app.provider_settings_scroll = 0;
    }} else {{
        // Sync scroll so the selected row is visible.
        let visible_rows = 12usize;
        let mut state = SelectListState::new(new_row_pos, app.provider_settings_scroll);
        state.sync_scroll(visible_rows);
        state.clamp_scroll(count, visible_rows);
        app.provider_settings_scroll = state.scroll();
    }}

    app.selected_provider_setting = new_row_pos.min(count.saturating_sub(1));
    if reset_filter {{
        app.provider_filter.clear();
    }}

    false
}}

'''

result = prefix + new_fn + suffix
with open(path, 'w') as f:
    f.write(result)
print('rewrote', len(s), '->', len(result))
