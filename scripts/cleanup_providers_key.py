path = 'crates/navi-tui/src/keybindings/modals.rs'
with open(path, 'r') as f:
    s = f.read()

# 1. Replace the awkward combined Enter|Char('k') arm with two simple arms.
old = '''        KeyCode::Enter | KeyCode::Char('k')
            if code == KeyCode::Enter
                || modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if let Some(provider) = provider_at(current_row_pos).cloned() {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') if modifiers.contains(KeyModifiers::CONTROL) => {'''
new = '''        KeyCode::Enter => {
            if let Some(provider) = provider_at(current_row_pos).cloned() {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(provider) = provider_at(current_row_pos).cloned() {
                app.pending_provider_setup = Some(provider.id.clone());
                app.pending_model_selection = None;
                app.api_key_input.clear();
                app.api_key_cursor = 0;
                super::apply_ui_effect(app, UiEffect::OpenModal(ModalKind::ApiKeyEntry));
            }
        }
        KeyCode::Char('o') | KeyCode::Char('O') if modifiers.contains(KeyModifiers::CONTROL) => {'''
assert old in s, 'old block not found'
s = s.replace(old, new, 1)

# 2. Drop the unused `reset_filter` variable.
old = '''    let mut new_row_pos = current_row_pos;
    let mut reset_to_first = false;
    let mut reset_filter = false;
'''
new = '''    let mut new_row_pos = current_row_pos;
    let mut reset_to_first = false;
'''
assert old in s, 'old var decl not found'
s = s.replace(old, new, 1)

# 3. Drop the dangling `reset_filter` consumer.
old = '''    app.selected_provider_setting = new_row_pos.min(count.saturating_sub(1));
    if reset_filter {
        app.provider_filter.clear();
    }

    false
}'''
new = '''    app.selected_provider_setting = new_row_pos.min(count.saturating_sub(1));

    false
}'''
assert old in s, 'old tail not found'
s = s.replace(old, new, 1)

with open(path, 'w') as f:
    f.write(s)
print('ok')
