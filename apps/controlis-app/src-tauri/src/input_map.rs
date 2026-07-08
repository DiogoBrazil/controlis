//! Translates browser input events (forwarded by the webview) into protocol
//! control messages, preserving the wire semantics of the old egui app:
//! typing travels as `Text`; named keys, modifiers and shortcuts as `KeyEvent`.
//! Unlike egui, the browser delivers explicit keydown/keyup for Shift/Ctrl/…,
//! so no modifier diffing is needed.

use protocol::{ControlMessage, KeyCode, MouseButton, PointerAction};
use serde::Deserialize;

/// An input event as produced by `public/js/screen.js` on the canvas.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UiInputEvent {
    /// Pointer position normalized to `0.0..=1.0` within the remote image.
    Move { x: f32, y: f32 },
    /// `button`: 0 = left, 1 = middle, 2 = right (browser convention).
    Button { button: u8, pressed: bool },
    /// Wheel deltas in "lines"; positive y = up (already flipped in JS).
    Wheel { dx: f32, dy: f32 },
    /// A named key or shortcut key (browser `event.key`), pressed or released.
    Key { key: String, pressed: bool },
    /// A run of typed printable characters (from the hidden input / IME).
    Text { text: String },
}

/// Maps a UI event to the control message to send, or `None` when the event
/// has no protocol equivalent (e.g. an unmapped key).
pub fn to_control_message(event: UiInputEvent) -> Option<ControlMessage> {
    match event {
        UiInputEvent::Move { x, y } => Some(ControlMessage::MouseMove {
            x_norm: x.clamp(0.0, 1.0),
            y_norm: y.clamp(0.0, 1.0),
        }),
        UiInputEvent::Button { button, pressed } => Some(ControlMessage::MouseButton {
            button: map_button(button)?,
            action: action(pressed),
        }),
        UiInputEvent::Wheel { dx, dy } => Some(ControlMessage::MouseWheel {
            delta_x: dx,
            delta_y: dy,
        }),
        UiInputEvent::Key { key, pressed } => Some(ControlMessage::KeyEvent {
            key: map_key(&key)?,
            action: action(pressed),
        }),
        UiInputEvent::Text { text } if !text.is_empty() => {
            Some(ControlMessage::Text { text })
        }
        UiInputEvent::Text { .. } => None,
    }
}

fn action(pressed: bool) -> PointerAction {
    if pressed {
        PointerAction::Press
    } else {
        PointerAction::Release
    }
}

fn map_button(button: u8) -> Option<MouseButton> {
    match button {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

/// Maps a browser `event.key` to a protocol key code. Single characters cover
/// shortcut letters (Ctrl+C arrives as key "c" with Control held separately).
fn map_key(key: &str) -> Option<KeyCode> {
    let code = match key {
        "Enter" => KeyCode::Enter,
        "Escape" => KeyCode::Escape,
        "Backspace" => KeyCode::Backspace,
        "Tab" => KeyCode::Tab,
        " " | "Spacebar" => KeyCode::Space,
        "Delete" => KeyCode::Delete,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "ArrowUp" => KeyCode::ArrowUp,
        "ArrowDown" => KeyCode::ArrowDown,
        "ArrowLeft" => KeyCode::ArrowLeft,
        "ArrowRight" => KeyCode::ArrowRight,
        "Shift" => KeyCode::Shift,
        "Control" => KeyCode::Control,
        "Alt" | "AltGraph" => KeyCode::Alt,
        "Meta" | "OS" => KeyCode::Meta,
        "CapsLock" => KeyCode::CapsLock,
        _ => {
            if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
                if (1..=12).contains(&n) {
                    return Some(KeyCode::Function(n));
                }
                return None;
            }
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            // Shortcut letters travel lowercase so the host maps the same
            // physical key regardless of Shift state.
            KeyCode::Unicode(c.to_ascii_lowercase())
        }
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_becomes_text() {
        let msg = to_control_message(UiInputEvent::Text { text: "ação çê".into() });
        assert_eq!(msg, Some(ControlMessage::Text { text: "ação çê".into() }));
    }

    #[test]
    fn empty_text_is_dropped() {
        assert_eq!(to_control_message(UiInputEvent::Text { text: String::new() }), None);
    }

    #[test]
    fn named_keys_and_modifiers_map() {
        let msg = to_control_message(UiInputEvent::Key { key: "Control".into(), pressed: true });
        assert_eq!(
            msg,
            Some(ControlMessage::KeyEvent {
                key: KeyCode::Control,
                action: PointerAction::Press
            })
        );
        let msg = to_control_message(UiInputEvent::Key { key: "F5".into(), pressed: false });
        assert_eq!(
            msg,
            Some(ControlMessage::KeyEvent {
                key: KeyCode::Function(5),
                action: PointerAction::Release
            })
        );
    }

    #[test]
    fn shortcut_letters_are_lowercased() {
        let msg = to_control_message(UiInputEvent::Key { key: "C".into(), pressed: true });
        assert_eq!(
            msg,
            Some(ControlMessage::KeyEvent {
                key: KeyCode::Unicode('c'),
                action: PointerAction::Press
            })
        );
    }

    #[test]
    fn coordinates_are_clamped() {
        let msg = to_control_message(UiInputEvent::Move { x: 1.5, y: -0.2 });
        assert_eq!(msg, Some(ControlMessage::MouseMove { x_norm: 1.0, y_norm: 0.0 }));
    }

    #[test]
    fn unknown_keys_are_dropped() {
        assert_eq!(
            to_control_message(UiInputEvent::Key { key: "MediaPlayPause".into(), pressed: true }),
            None
        );
    }
}
