//! Input simulation via `CGEvent`.
//!
//! Synthesizes mouse and keyboard events. Requires Accessibility
//! permission (and Input Monitoring for some event types).

use anyhow::{Result, anyhow, bail};
use serde_json::Value;

use crate::MacInputResult;

/// Simulates a sequence of input actions (mouse + keyboard).
///
/// Action JSON format (same as Windows backend):
/// - `{"action":"mouse_move","x":100,"y":200}`
/// - `{"action":"click","button":"left","x":100,"y":200}`
/// - `{"action":"double_click","button":"left","x":100,"y":200}`
/// - `{"action":"scroll","delta":-1,"x":100,"y":200}`
/// - `{"action":"key","key":"Enter"}`
/// - `{"action":"key_down","key":"Shift"}`
/// - `{"action":"key_up","key":"Shift"}`
/// - `{"action":"type","text":"hello world"}`
pub fn simulate_input(actions: &[Value]) -> Result<MacInputResult> {
    unsafe {
        // Check Accessibility permission.
        if !accessibility_sys::AXIsProcessTrusted() {
            bail!(
                "macOS Accessibility permission not granted. \
                 Grant it in System Settings → Privacy & Security → Accessibility."
            );
        }

        let mut performed = 0usize;
        for action in actions {
            let action_type = action.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let result = match action_type {
                "mouse_move" => do_mouse_move(action),
                "click" => do_click(action, false),
                "double_click" => do_click(action, true),
                "scroll" => do_scroll(action),
                "key" => do_key(action, true, true),
                "key_down" => do_key(action, true, false),
                "key_up" => do_key(action, false, true),
                "type" => do_type(action),
                "" => Err(anyhow!("missing 'action' field")),
                other => Err(anyhow!("unknown action: {other}")),
            };
            result?;
            performed += 1;
        }

        Ok(MacInputResult {
            actions_performed: performed,
        })
    }
}

unsafe fn do_mouse_move(action: &Value) -> Result<()> {
    let x = action
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow!("mouse_move: missing 'x'"))?;
    let y = action
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow!("mouse_move: missing 'y'"))?;

    let event = core_graphics::event::CGEvent::new_mouse_event(
        core_graphics::event::CGEventSource::new(
            core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
        )
        .map_err(|e| anyhow!("CGEventSource failed: {e:?}"))?,
        core_graphics::event::CGEventType::MouseMoved,
        core_graphics::geometry::CGPoint { x, y },
        core_graphics::event::CGMouseButton::Left,
    )
    .map_err(|e| anyhow!("CGEvent creation failed: {e:?}"))?;

    event.post(core_graphics::event::CGEventTapLocation::HID);
    Ok(())
}

unsafe fn do_click(action: &Value, double: bool) -> Result<()> {
    let x = action
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow!("click: missing 'x'"))?;
    let y = action
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow!("click: missing 'y'"))?;
    let button = action
        .get("button")
        .and_then(|v| v.as_str())
        .unwrap_or("left");

    let (cg_button, down_event, up_event) = match button {
        "left" => (
            core_graphics::event::CGMouseButton::Left,
            core_graphics::event::CGEventType::LeftMouseDown,
            core_graphics::event::CGEventType::LeftMouseUp,
        ),
        "right" => (
            core_graphics::event::CGMouseButton::Right,
            core_graphics::event::CGEventType::RightMouseDown,
            core_graphics::event::CGEventType::RightMouseUp,
        ),
        "middle" => (
            core_graphics::event::CGMouseButton::Center,
            core_graphics::event::CGEventType::OtherMouseDown,
            core_graphics::event::CGEventType::OtherMouseUp,
        ),
        other => return Err(anyhow!("unknown button: {other}")),
    };

    let source = core_graphics::event::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
    )
    .map_err(|e| anyhow!("CGEventSource failed: {e:?}"))?;

    let point = core_graphics::geometry::CGPoint { x, y };

    // Move to position first.
    let move_event = core_graphics::event::CGEvent::new_mouse_event(
        source.clone(),
        core_graphics::event::CGEventType::MouseMoved,
        point,
        cg_button,
    )
    .map_err(|e| anyhow!("CGEvent move failed: {e:?}"))?;
    move_event.post(core_graphics::event::CGEventTapLocation::HID);

    // Mouse down.
    let down = core_graphics::event::CGEvent::new_mouse_event(
        source.clone(),
        down_event,
        point,
        cg_button,
    )
    .map_err(|e| anyhow!("CGEvent down failed: {e:?}"))?;
    down.post(core_graphics::event::CGEventTapLocation::HID);

    // Mouse up.
    let up =
        core_graphics::event::CGEvent::new_mouse_event(source.clone(), up_event, point, cg_button)
            .map_err(|e| anyhow!("CGEvent up failed: {e:?}"))?;
    up.post(core_graphics::event::CGEventTapLocation::HID);

    // Double click: send another down/up with click count set.
    if double {
        let down2 = core_graphics::event::CGEvent::new_mouse_event(
            source.clone(),
            down_event,
            point,
            cg_button,
        )
        .map_err(|e| anyhow!("CGEvent down2 failed: {e:?}"))?;
        down2.set_integer_value_field(core_graphics::event::CGEventField::MouseEventClickState, 2);
        down2.post(core_graphics::event::CGEventTapLocation::HID);

        let up2 =
            core_graphics::event::CGEvent::new_mouse_event(source, up_event, point, cg_button)
                .map_err(|e| anyhow!("CGEvent up2 failed: {e:?}"))?;
        up2.set_integer_value_field(core_graphics::event::CGEventField::MouseEventClickState, 2);
        up2.post(core_graphics::event::CGEventTapLocation::HID);
    }

    Ok(())
}

unsafe fn do_scroll(action: &Value) -> Result<()> {
    let delta = action
        .get("delta")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("scroll: missing 'delta'"))?;
    let x = action.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = action.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let source = core_graphics::event::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
    )
    .map_err(|e| anyhow!("CGEventSource failed: {e:?}"))?;

    // Move to position first.
    let move_event = core_graphics::event::CGEvent::new_mouse_event(
        source.clone(),
        core_graphics::event::CGEventType::MouseMoved,
        core_graphics::geometry::CGPoint { x, y },
        core_graphics::event::CGMouseButton::Left,
    )
    .map_err(|e| anyhow!("CGEvent move failed: {e:?}"))?;
    move_event.post(core_graphics::event::CGEventTapLocation::HID);

    // Create scroll event.
    let scroll_event = core_graphics::event::CGEvent::new_scroll_event(
        source,
        core_graphics::event::ScrollEventUnit::LINE,
        1,            // wheel count
        delta as i32, // vertical delta
        0,            // horizontal delta
        0,            // delta axis 2
    )
    .map_err(|e| anyhow!("CGEvent scroll failed: {e:?}"))?;
    scroll_event.post(core_graphics::event::CGEventTapLocation::HID);

    Ok(())
}

unsafe fn do_key(action: &Value, down: bool, up: bool) -> Result<()> {
    let key = action
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("key: missing 'key'"))?;

    let keycode = map_key_to_keycode(key).ok_or_else(|| anyhow!("unknown key: {key}"))?;

    let source = core_graphics::event::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
    )
    .map_err(|e| anyhow!("CGEventSource failed: {e:?}"))?;

    if down {
        let event =
            core_graphics::event::CGEvent::new_keyboard_event(source.clone(), keycode, true)
                .map_err(|e| anyhow!("CGEvent key_down failed: {e:?}"))?;
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }

    if up {
        let event = core_graphics::event::CGEvent::new_keyboard_event(source, keycode, false)
            .map_err(|e| anyhow!("CGEvent key_up failed: {e:?}"))?;
        event.post(core_graphics::event::CGEventTapLocation::HID);
    }

    Ok(())
}

unsafe fn do_type(action: &Value) -> Result<()> {
    let text = action
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("type: missing 'text'"))?;

    let source = core_graphics::event::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::HIDSystemState,
    )
    .map_err(|e| anyhow!("CGEventSource failed: {e:?}"))?;

    for ch in text.chars() {
        // Create a key-down event with the Unicode character.
        let down = core_graphics::event::CGEvent::new_keyboard_event(
            source.clone(),
            0, // virtual key (0 = use Unicode string)
            true,
        )
        .map_err(|e| anyhow!("CGEvent type_down failed: {e:?}"))?;
        down.set_string_from_utf8(&[ch]);
        down.post(core_graphics::event::CGEventTapLocation::HID);

        // Key-up.
        let up = core_graphics::event::CGEvent::new_keyboard_event(source.clone(), 0, false)
            .map_err(|e| anyhow!("CGEvent type_up failed: {e:?}"))?;
        up.set_string_from_utf8(&[ch]);
        up.post(core_graphics::event::CGEventTapLocation::HID);
    }

    Ok(())
}

/// Maps a named key or single character to a macOS virtual keycode.
fn map_key_to_keycode(key: &str) -> Option<u16> {
    // macOS virtual keycodes (from Carbon/HIToolbox/Events.h).
    match key {
        "Enter" | "Return" => Some(36),
        "Tab" => Some(48),
        "Escape" | "Esc" => Some(53),
        "Backspace" | "Delete" => Some(51),
        "ForwardDelete" => Some(117),
        "Space" => Some(49),
        "Home" => Some(115),
        "End" => Some(119),
        "PageUp" => Some(116),
        "PageDown" => Some(121),
        "ArrowUp" | "Up" => Some(126),
        "ArrowDown" | "Down" => Some(125),
        "ArrowLeft" | "Left" => Some(123),
        "ArrowRight" | "Right" => Some(124),
        "Shift" => Some(56),
        "Control" | "Ctrl" => Some(59),
        "Option" | "Alt" => Some(58),
        "Command" | "Cmd" | "Win" | "Meta" => Some(55),
        "CapsLock" => Some(57),
        "F1" => Some(122),
        "F2" => Some(120),
        "F3" => Some(99),
        "F4" => Some(118),
        "F5" => Some(96),
        "F6" => Some(97),
        "F7" => Some(98),
        "F8" => Some(100),
        "F9" => Some(101),
        "F10" => Some(109),
        "F11" => Some(103),
        "F12" => Some(111),
        _ => {
            // Single character: map to keycode.
            let ch = key.chars().next()?;
            Some(char_to_keycode(ch))
        }
    }
}

/// Maps a single character to a macOS virtual keycode (US keyboard layout).
fn char_to_keycode(ch: char) -> u16 {
    match ch {
        'a' | 'A' => 0,
        'b' | 'B' => 11,
        'c' | 'C' => 8,
        'd' | 'D' => 2,
        'e' | 'E' => 14,
        'f' | 'F' => 3,
        'g' | 'G' => 5,
        'h' | 'H' => 4,
        'i' | 'I' => 34,
        'j' | 'J' => 38,
        'k' | 'K' => 40,
        'l' | 'L' => 37,
        'm' | 'M' => 46,
        'n' | 'N' => 45,
        'o' | 'O' => 31,
        'p' | 'P' => 35,
        'q' | 'Q' => 12,
        'r' | 'R' => 15,
        's' | 'S' => 1,
        't' | 'T' => 17,
        'u' | 'U' => 32,
        'v' | 'V' => 9,
        'w' | 'W' => 13,
        'x' | 'X' => 7,
        'y' | 'Y' => 16,
        'z' | 'Z' => 6,
        '0' => 29,
        '1' => 18,
        '2' => 19,
        '3' => 20,
        '4' => 21,
        '5' => 23,
        '6' => 22,
        '7' => 26,
        '8' => 28,
        '9' => 25,
        '-' => 27,
        '=' => 24,
        '[' => 33,
        ']' => 30,
        '\\' => 42,
        ';' => 41,
        '\'' => 39,
        '`' => 50,
        ',' => 43,
        '.' => 47,
        '/' => 44,
        ' ' => 49,
        _ => 0, // Fallback — will use Unicode string instead.
    }
}
