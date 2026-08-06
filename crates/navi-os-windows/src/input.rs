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
