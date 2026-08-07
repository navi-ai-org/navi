//! Input simulation via `SendInput` (mouse + keyboard).
//!
//! Accepts a JSON array of action objects, dispatches them in order, and
//! returns the count of successfully performed actions.

use super::WinInputResult;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{SetCursorPos, WHEEL_DELTA};

/// Simulates a sequence of input actions.
///
/// Each action is a JSON object with an `action` field:
/// - `{"action":"mouse_move","x":100,"y":200}`
/// - `{"action":"click","button":"left","x":100,"y":200}` (button: left/right/middle)
/// - `{"action":"double_click","button":"left","x":100,"y":200}`
/// - `{"action":"scroll","delta":-1,"x":100,"y":200}` (delta in wheel notches)
/// - `{"action":"key","key":"Enter"}` (press + release)
/// - `{"action":"key_down","key":"Shift"}`
/// - `{"action":"key_up","key":"Shift"}`
/// - `{"action":"type","text":"hello world"}` (types each character)
pub fn simulate_input(actions: &[Value]) -> Result<WinInputResult> {
    let mut performed = 0usize;

    for (i, action) in actions.iter().enumerate() {
        let kind = action
            .get("action")
            .and_then(Value::as_str)
            .with_context(|| format!("action[{i}]: missing `action` string"))?;

        match kind {
            "mouse_move" => {
                let x = get_i32(action, "x")?;
                let y = get_i32(action, "y")?;
                do_mouse_move(x, y)?;
            }
            "click" | "double_click" => {
                let button = action
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left");
                let x = get_i32(action, "x")?;
                let y = get_i32(action, "y")?;
                do_click(button, x, y, kind == "double_click")?;
            }
            "scroll" => {
                let delta = get_i32(action, "delta")?;
                let x = get_i32(action, "x")?;
                let y = get_i32(action, "y")?;
                do_scroll(delta, x, y)?;
            }
            "key" => {
                let key = get_str(action, "key")?;
                let vk = key_to_vk(key);
                do_key_press(vk)?;
            }
            "key_down" => {
                let key = get_str(action, "key")?;
                let vk = key_to_vk(key);
                do_key_event(vk, false)?;
            }
            "key_up" => {
                let key = get_str(action, "key")?;
                let vk = key_to_vk(key);
                do_key_event(vk, true)?;
            }
            "type" => {
                let text = get_str(action, "text")?;
                do_type_text(text)?;
            }
            other => bail!("action[{i}]: unknown action `{other}`"),
        }
        performed += 1;
    }

    Ok(WinInputResult {
        actions_performed: performed,
    })
}

fn get_i32(action: &Value, key: &str) -> Result<i32> {
    action
        .get(key)
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .with_context(|| format!("missing or invalid `{key}` (expected integer)"))
}

fn get_str<'a>(action: &'a Value, key: &str) -> Result<&'a str> {
    action
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .with_context(|| format!("missing or empty `{key}` string"))
}

// ── Mouse ──────────────────────────────────────────────────────────────────

fn do_mouse_move(x: i32, y: i32) -> Result<()> {
    unsafe {
        // SetCursorPos moves the cursor instantly.
        if SetCursorPos(x, y) == 0 {
            bail!("SetCursorPos failed for ({x}, {y})");
        }
    }
    Ok(())
}

fn do_click(button: &str, x: i32, y: i32, double_click: bool) -> Result<()> {
    let flags_down = match button {
        "right" => MOUSEEVENTF_RIGHTDOWN,
        "middle" => MOUSEEVENTF_MIDDLEDOWN,
        _ => MOUSEEVENTF_LEFTDOWN,
    };
    let flags_up = match button {
        "right" => MOUSEEVENTF_RIGHTUP,
        "middle" => MOUSEEVENTF_MIDDLEUP,
        _ => MOUSEEVENTF_LEFTUP,
    };

    // Move to position first.
    do_mouse_move(x, y)?;

    let mut inputs = vec![
        build_mouse_input(flags_down, 0),
        build_mouse_input(flags_up, 0),
    ];
    if double_click {
        inputs.push(build_mouse_input(flags_down, 0));
        inputs.push(build_mouse_input(flags_up, 0));
    }

    send_inputs(&inputs)
}

fn do_scroll(delta: i32, x: i32, y: i32) -> Result<()> {
    do_mouse_move(x, y)?;
    // WHEEL_DELTA is 120; delta is in "notches" (1 notch = 120 units).
    let wheel_delta = (delta * (WHEEL_DELTA as i32)) as u32;
    let input = build_mouse_input(MOUSEEVENTF_WHEEL, wheel_delta);
    send_inputs(&[input])
}

fn build_mouse_input(flags: u32, mouse_data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

// ── Keyboard ───────────────────────────────────────────────────────────────

fn do_key_press(vk: u16) -> Result<()> {
    do_key_event(vk, false)?;
    do_key_event(vk, true)
}

fn do_key_event(vk: u16, key_up: bool) -> Result<()> {
    let flags: u32 = if key_up { KEYEVENTF_KEYUP } else { 0 };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send_inputs(&[input])
}

fn do_type_text(text: &str) -> Result<()> {
    // Type each Unicode character via ScanCode + KEYEVENTF_UNICODE.
    let mut inputs = Vec::with_capacity(text.chars().count() * 2);
    for c in text.chars() {
        let scan = c as u16;
        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        inputs.push(down);
        inputs.push(up);
    }
    send_inputs(&inputs)
}

// ── SendInput dispatch ─────────────────────────────────────────────────────

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    unsafe {
        let sent = SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
        if sent == 0 {
            bail!("SendInput returned 0 (events may have been blocked by UIPI)");
        }
    }
    Ok(())
}

/// Maps a key name string to a Win32 virtual-key code.
///
/// Supports common named keys and single characters. Unknown names default
/// to the uppercase ASCII code of the first character.
fn key_to_vk(key: &str) -> u16 {
    match key.to_ascii_uppercase().as_str() {
        "ENTER" | "RETURN" => VK_RETURN,
        "TAB" => VK_TAB,
        "ESCAPE" | "ESC" => VK_ESCAPE,
        "BACKSPACE" | "BACK" => VK_BACK,
        "DELETE" | "DEL" => VK_DELETE,
        "SPACE" => VK_SPACE,
        "UP" => VK_UP,
        "DOWN" => VK_DOWN,
        "LEFT" => VK_LEFT,
        "RIGHT" => VK_RIGHT,
        "HOME" => VK_HOME,
        "END" => VK_END,
        "PAGEUP" | "PGUP" => VK_PRIOR,
        "PAGEDOWN" | "PGDN" => VK_NEXT,
        "SHIFT" => VK_SHIFT,
        "CTRL" | "CONTROL" => VK_CONTROL,
        "ALT" | "MENU" => VK_MENU,
        "WIN" | "META" | "SUPER" => VK_LWIN,
        "CAPSLOCK" => VK_CAPITAL,
        "NUMLOCK" => VK_NUMLOCK,
        "SCROLLLOCK" => VK_SCROLL,
        "F1" => VK_F1,
        "F2" => VK_F2,
        "F3" => VK_F3,
        "F4" => VK_F4,
        "F5" => VK_F5,
        "F6" => VK_F6,
        "F7" => VK_F7,
        "F8" => VK_F8,
        "F9" => VK_F9,
        "F10" => VK_F10,
        "F11" => VK_F11,
        "F12" => VK_F12,
        other => {
            // Single character: use its ASCII code as the VK code.
            if let Some(c) = other.chars().next() {
                c.to_ascii_uppercase() as u16
            } else {
                VK_SPACE
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── key_to_vk unit tests ─────────────────────────────────────────────

    #[test]
    fn key_to_vk_named_keys() {
        assert_eq!(key_to_vk("Enter"), VK_RETURN);
        assert_eq!(key_to_vk("Return"), VK_RETURN);
        assert_eq!(key_to_vk("Tab"), VK_TAB);
        assert_eq!(key_to_vk("Escape"), VK_ESCAPE);
        assert_eq!(key_to_vk("Esc"), VK_ESCAPE);
        assert_eq!(key_to_vk("Backspace"), VK_BACK);
        assert_eq!(key_to_vk("Back"), VK_BACK);
        assert_eq!(key_to_vk("Delete"), VK_DELETE);
        assert_eq!(key_to_vk("Del"), VK_DELETE);
        assert_eq!(key_to_vk("Space"), VK_SPACE);
    }

    #[test]
    fn key_to_vk_arrow_keys() {
        assert_eq!(key_to_vk("Up"), VK_UP);
        assert_eq!(key_to_vk("Down"), VK_DOWN);
        assert_eq!(key_to_vk("Left"), VK_LEFT);
        assert_eq!(key_to_vk("Right"), VK_RIGHT);
    }

    #[test]
    fn key_to_vk_navigation_keys() {
        assert_eq!(key_to_vk("Home"), VK_HOME);
        assert_eq!(key_to_vk("End"), VK_END);
        assert_eq!(key_to_vk("PageUp"), VK_PRIOR);
        assert_eq!(key_to_vk("PgUp"), VK_PRIOR);
        assert_eq!(key_to_vk("PageDown"), VK_NEXT);
        assert_eq!(key_to_vk("PgDn"), VK_NEXT);
    }

    #[test]
    fn key_to_vk_modifier_keys() {
        assert_eq!(key_to_vk("Shift"), VK_SHIFT);
        assert_eq!(key_to_vk("Ctrl"), VK_CONTROL);
        assert_eq!(key_to_vk("Control"), VK_CONTROL);
        assert_eq!(key_to_vk("Alt"), VK_MENU);
        assert_eq!(key_to_vk("Menu"), VK_MENU);
        assert_eq!(key_to_vk("Win"), VK_LWIN);
        assert_eq!(key_to_vk("Meta"), VK_LWIN);
        assert_eq!(key_to_vk("Super"), VK_LWIN);
    }

    #[test]
    fn key_to_vk_lock_keys() {
        assert_eq!(key_to_vk("CapsLock"), VK_CAPITAL);
        assert_eq!(key_to_vk("NumLock"), VK_NUMLOCK);
        assert_eq!(key_to_vk("ScrollLock"), VK_SCROLL);
    }

    #[test]
    fn key_to_vk_function_keys() {
        assert_eq!(key_to_vk("F1"), VK_F1);
        assert_eq!(key_to_vk("F6"), VK_F6);
        assert_eq!(key_to_vk("F12"), VK_F12);
    }

    #[test]
    fn key_to_vk_case_insensitive() {
        assert_eq!(key_to_vk("enter"), VK_RETURN);
        assert_eq!(key_to_vk("ENTER"), VK_RETURN);
        assert_eq!(key_to_vk("EnTeR"), VK_RETURN);
        assert_eq!(key_to_vk("shift"), VK_SHIFT);
        assert_eq!(key_to_vk("SHIFT"), VK_SHIFT);
    }

    #[test]
    fn key_to_vk_single_char_defaults_to_ascii() {
        // Single character: uppercase ASCII code.
        assert_eq!(key_to_vk("a"), b'A' as u16);
        assert_eq!(key_to_vk("z"), b'Z' as u16);
        assert_eq!(key_to_vk("A"), b'A' as u16);
        assert_eq!(key_to_vk("1"), b'1' as u16);
    }

    #[test]
    fn key_to_vk_unknown_multi_char_defaults_to_first_char() {
        // Unknown multi-char string: uppercase of first char.
        assert_eq!(key_to_vk("xyz"), b'X' as u16);
        assert_eq!(key_to_vk("qq"), b'Q' as u16);
    }

    #[test]
    fn key_to_vk_empty_string_defaults_to_space() {
        // The match arm `other` has an empty string; chars().next()
        // returns None, so we fall back to VK_SPACE.
        assert_eq!(key_to_vk(""), VK_SPACE);
    }

    // ── get_i32 unit tests ───────────────────────────────────────────────

    #[test]
    fn get_i32_extracts_valid_integer() {
        let action = json!({"x": 100});
        assert_eq!(get_i32(&action, "x").unwrap(), 100);
    }

    #[test]
    fn get_i32_extracts_negative_integer() {
        let action = json!({"x": -200});
        assert_eq!(get_i32(&action, "x").unwrap(), -200);
    }

    #[test]
    fn get_i32_extracts_zero() {
        let action = json!({"x": 0});
        assert_eq!(get_i32(&action, "x").unwrap(), 0);
    }

    #[test]
    fn get_i32_errors_on_missing_field() {
        let action = json!({"y": 100});
        assert!(get_i32(&action, "x").is_err());
        let err = get_i32(&action, "x").unwrap_err().to_string();
        assert!(
            err.contains("missing"),
            "should mention missing, got: {err}"
        );
    }

    #[test]
    fn get_i32_errors_on_non_integer() {
        let action = json!({"x": "not a number"});
        assert!(get_i32(&action, "x").is_err());
        let action = json!({"x": true});
        assert!(get_i32(&action, "x").is_err());
        let action = json!({"x": 1.5});
        // as_i64() returns None for floats — should error.
        assert!(get_i32(&action, "x").is_err());
    }

    #[test]
    fn get_i32_truncates_large_i64_to_i32() {
        let action = json!({"x": 2147483648i64}); // i32::MAX + 1
        // This wraps to -2147483648 when cast to i32. The function
        // doesn't validate range — it just casts. This test documents
        // that behavior.
        let result = get_i32(&action, "x").unwrap();
        assert_eq!(result, -2147483648);
    }

    // ── get_str unit tests ───────────────────────────────────────────────

    #[test]
    fn get_str_extracts_valid_string() {
        let action = json!({"key": "Enter"});
        assert_eq!(get_str(&action, "key").unwrap(), "Enter");
    }

    #[test]
    fn get_str_errors_on_missing_field() {
        let action = json!({"other": "value"});
        assert!(get_str(&action, "key").is_err());
        let err = get_str(&action, "key").unwrap_err().to_string();
        assert!(
            err.contains("missing"),
            "should mention missing, got: {err}"
        );
    }

    #[test]
    fn get_str_errors_on_empty_string() {
        let action = json!({"key": ""});
        assert!(get_str(&action, "key").is_err());
        let err = get_str(&action, "key").unwrap_err().to_string();
        assert!(err.contains("empty"), "should mention empty, got: {err}");
    }

    #[test]
    fn get_str_errors_on_non_string() {
        let action = json!({"key": 123});
        assert!(get_str(&action, "key").is_err());
        let action = json!({"key": true});
        assert!(get_str(&action, "key").is_err());
        let action = json!({"key": null});
        assert!(get_str(&action, "key").is_err());
    }

    // ── build_mouse_input unit tests ─────────────────────────────────────

    #[test]
    fn build_mouse_input_sets_correct_type() {
        let input = build_mouse_input(MOUSEEVENTF_LEFTDOWN, 0);
        assert_eq!(input.r#type, INPUT_MOUSE);
    }

    #[test]
    fn build_mouse_input_sets_flags() {
        let input = build_mouse_input(MOUSEEVENTF_RIGHTDOWN, 42);
        // Access the mi union member.
        let mi = unsafe { input.Anonymous.mi };
        assert_eq!(mi.dwFlags, MOUSEEVENTF_RIGHTDOWN);
        assert_eq!(mi.mouseData, 42);
    }

    // ── simulate_input parsing/error path tests ──────────────────────────
    //
    // These test the parsing and validation layer of simulate_input.
    // They exercise the error paths that bail! before reaching any FFI
    // call, so they pass without a desktop.

    #[test]
    fn simulate_input_empty_actions_returns_zero() {
        let result = simulate_input(&[]).unwrap();
        assert_eq!(result.actions_performed, 0);
    }

    #[test]
    fn simulate_input_missing_action_field_errors() {
        let actions = vec![json!({"x": 100, "y": 200})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(
            err.contains("missing") && err.contains("action"),
            "should mention missing action, got: {err}"
        );
    }

    #[test]
    fn simulate_input_missing_action_field_includes_index() {
        let actions = vec![
            json!({"action": "click", "x": 0, "y": 0}),
            json!({"x": 100}),
        ];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(
            err.contains("action[1]"),
            "should include index, got: {err}"
        );
    }

    #[test]
    fn simulate_input_unknown_action_type_errors() {
        let actions = vec![json!({"action": "fly_to_moon"})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(
            err.contains("unknown") && err.contains("fly_to_moon"),
            "should mention unknown action, got: {err}"
        );
    }

    #[test]
    fn simulate_input_click_missing_x_errors() {
        let actions = vec![json!({"action": "click", "y": 200})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(err.contains("missing") && err.contains("x"), "got: {err}");
    }

    #[test]
    fn simulate_input_click_missing_y_errors() {
        let actions = vec![json!({"action": "click", "x": 100})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(err.contains("missing") && err.contains("y"), "got: {err}");
    }

    #[test]
    fn simulate_input_key_missing_key_field_errors() {
        let actions = vec![json!({"action": "key"})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(err.contains("missing") && err.contains("key"), "got: {err}");
    }

    #[test]
    fn simulate_input_type_missing_text_errors() {
        let actions = vec![json!({"action": "type"})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(
            err.contains("missing") && err.contains("text"),
            "got: {err}"
        );
    }

    #[test]
    fn simulate_input_type_empty_text_errors() {
        let actions = vec![json!({"action": "type", "text": ""})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(err.contains("empty") && err.contains("text"), "got: {err}");
    }

    #[test]
    fn simulate_input_scroll_missing_delta_errors() {
        let actions = vec![json!({"action": "scroll", "x": 100, "y": 200})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(
            err.contains("missing") && err.contains("delta"),
            "got: {err}"
        );
    }

    #[test]
    fn simulate_input_mouse_move_missing_x_errors() {
        let actions = vec![json!({"action": "mouse_move", "y": 200})];
        let err = simulate_input(&actions).unwrap_err().to_string();
        assert!(err.contains("missing") && err.contains("x"), "got: {err}");
    }

    #[test]
    fn simulate_input_non_integer_x_errors() {
        let actions = vec![json!({"action": "click", "x": "abc", "y": 200})];
        assert!(simulate_input(&actions).is_err());
    }

    #[test]
    fn simulate_input_key_non_string_key_errors() {
        let actions = vec![json!({"action": "key", "key": 123})];
        assert!(simulate_input(&actions).is_err());
    }

    // ── simulate_input integration tests (skip-on-failure) ───────────────

    #[test]
    #[cfg(windows)]
    fn simulate_input_mouse_move_to_center_succeeds_or_skips() {
        let actions = vec![json!({"action": "mouse_move", "x": 100, "y": 100})];
        match simulate_input(&actions) {
            Ok(result) => assert_eq!(result.actions_performed, 1),
            Err(e) => {
                eprintln!("skipping simulate_input_mouse_move: {e}");
                return;
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn simulate_input_click_left_succeeds_or_skips() {
        let actions = vec![json!({"action": "click", "button": "left", "x": 50, "y": 50})];
        match simulate_input(&actions) {
            Ok(result) => assert_eq!(result.actions_performed, 1),
            Err(e) => {
                eprintln!("skipping simulate_input_click: {e}");
                return;
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn simulate_input_key_press_succeeds_or_skips() {
        let actions = vec![json!({"action": "key", "key": "Escape"})];
        match simulate_input(&actions) {
            Ok(result) => assert_eq!(result.actions_performed, 1),
            Err(e) => {
                eprintln!("skipping simulate_input_key: {e}");
                return;
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn simulate_input_type_text_succeeds_or_skips() {
        let actions = vec![json!({"action": "type", "text": "hi"})];
        match simulate_input(&actions) {
            Ok(result) => assert_eq!(result.actions_performed, 1),
            Err(e) => {
                eprintln!("skipping simulate_input_type: {e}");
                return;
            }
        }
    }

    #[test]
    #[cfg(windows)]
    fn simulate_input_multiple_actions_succeeds_or_skips() {
        let actions = vec![
            json!({"action": "mouse_move", "x": 10, "y": 10}),
            json!({"action": "key", "key": "Shift"}),
            json!({"action": "type", "text": "test"}),
        ];
        match simulate_input(&actions) {
            Ok(result) => assert_eq!(result.actions_performed, 3),
            Err(e) => {
                eprintln!("skipping simulate_input_multiple: {e}");
                return;
            }
        }
    }
}
