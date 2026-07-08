//! Translates egui input into protocol control messages for the viewer.
//!
//! Typing goes through egui `Text` events (which already apply the viewer's layout
//! and shift state). Non-text keys and modifier-driven shortcuts go through `Key`
//! events. Modifier transitions are diffed each frame so shortcuts like Ctrl+C work
//! even though no `Text` event fires for them.

use egui::{Event, Key, PointerButton, Pos2, Rect};
use protocol::{ControlMessage, KeyCode, MouseButton, PointerAction};
use session_viewer::ViewerHandle;

/// Per-frame state needed to diff modifiers between updates.
#[derive(Default)]
pub struct InputState {
    modifiers: egui::Modifiers,
}

/// Forwards all input over the image area to the host.
pub fn forward(
    ui: &egui::Ui,
    image_rect: Rect,
    handle: &ViewerHandle,
    state: &mut InputState,
) {
    let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
    diff_modifiers(state.modifiers, modifiers, handle);
    state.modifiers = modifiers;

    for event in events {
        match event {
            Event::PointerMoved(pos) => send_move(handle, image_rect, pos),
            Event::PointerButton { pos, button, pressed, .. } => {
                send_move(handle, image_rect, pos);
                if let Some(button) = map_button(button) {
                    handle.send_input(ControlMessage::MouseButton {
                        button,
                        action: action(pressed),
                    });
                }
            }
            Event::MouseWheel { delta, .. } => {
                handle.send_input(ControlMessage::MouseWheel {
                    delta_x: delta.x,
                    delta_y: delta.y,
                });
            }
            Event::Text(text) => {
                if !modifiers.ctrl && !modifiers.alt && !modifiers.command {
                    handle.send_input(ControlMessage::Text { text });
                }
            }
            Event::Key { key, pressed, modifiers: m, .. } => {
                if let Some(code) = map_key(key) {
                    let shortcut = m.ctrl || m.alt || m.command;
                    if is_non_text(key) || shortcut {
                        handle.send_input(ControlMessage::KeyEvent {
                            key: code,
                            action: action(pressed),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn send_move(handle: &ViewerHandle, rect: Rect, pos: Pos2) {
    if let Some((x, y)) = normalize(rect, pos) {
        handle.send_input(ControlMessage::MouseMove { x_norm: x, y_norm: y });
    }
}

fn normalize(rect: Rect, pos: Pos2) -> Option<(f32, f32)> {
    if !rect.contains(pos) || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let x = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let y = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    Some((x, y))
}

fn action(pressed: bool) -> PointerAction {
    if pressed {
        PointerAction::Press
    } else {
        PointerAction::Release
    }
}

fn map_button(button: PointerButton) -> Option<MouseButton> {
    match button {
        PointerButton::Primary => Some(MouseButton::Left),
        PointerButton::Secondary => Some(MouseButton::Right),
        PointerButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

fn diff_modifiers(prev: egui::Modifiers, now: egui::Modifiers, handle: &ViewerHandle) {
    let transitions = [
        (prev.ctrl, now.ctrl, KeyCode::Control),
        (prev.alt, now.alt, KeyCode::Alt),
        (prev.shift, now.shift, KeyCode::Shift),
        (prev.command, now.command, KeyCode::Meta),
    ];
    for (was, is, code) in transitions {
        if was != is {
            handle.send_input(ControlMessage::KeyEvent {
                key: code,
                action: action(is),
            });
        }
    }
}

/// Keys that do not produce a `Text` event and must be forwarded via `Key`.
fn is_non_text(key: Key) -> bool {
    matches!(
        key,
        Key::Enter
            | Key::Escape
            | Key::Backspace
            | Key::Tab
            | Key::Delete
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::ArrowLeft
            | Key::ArrowRight
            | Key::F1
            | Key::F2
            | Key::F3
            | Key::F4
            | Key::F5
            | Key::F6
            | Key::F7
            | Key::F8
            | Key::F9
            | Key::F10
            | Key::F11
            | Key::F12
    )
}

fn map_key(key: Key) -> Option<KeyCode> {
    let code = match key {
        Key::Enter => KeyCode::Enter,
        Key::Escape => KeyCode::Escape,
        Key::Backspace => KeyCode::Backspace,
        Key::Tab => KeyCode::Tab,
        Key::Space => KeyCode::Space,
        Key::Delete => KeyCode::Delete,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::ArrowUp => KeyCode::ArrowUp,
        Key::ArrowDown => KeyCode::ArrowDown,
        Key::ArrowLeft => KeyCode::ArrowLeft,
        Key::ArrowRight => KeyCode::ArrowRight,
        Key::F1 => KeyCode::Function(1),
        Key::F2 => KeyCode::Function(2),
        Key::F3 => KeyCode::Function(3),
        Key::F4 => KeyCode::Function(4),
        Key::F5 => KeyCode::Function(5),
        Key::F6 => KeyCode::Function(6),
        Key::F7 => KeyCode::Function(7),
        Key::F8 => KeyCode::Function(8),
        Key::F9 => KeyCode::Function(9),
        Key::F10 => KeyCode::Function(10),
        Key::F11 => KeyCode::Function(11),
        Key::F12 => KeyCode::Function(12),
        other => KeyCode::Unicode(letter_or_digit(other)?),
    };
    Some(code)
}

/// Maps letter/digit keys to their base character for modifier shortcuts.
fn letter_or_digit(key: Key) -> Option<char> {
    let c = match key {
        Key::A => 'a', Key::B => 'b', Key::C => 'c', Key::D => 'd', Key::E => 'e',
        Key::F => 'f', Key::G => 'g', Key::H => 'h', Key::I => 'i', Key::J => 'j',
        Key::K => 'k', Key::L => 'l', Key::M => 'm', Key::N => 'n', Key::O => 'o',
        Key::P => 'p', Key::Q => 'q', Key::R => 'r', Key::S => 's', Key::T => 't',
        Key::U => 'u', Key::V => 'v', Key::W => 'w', Key::X => 'x', Key::Y => 'y',
        Key::Z => 'z',
        Key::Num0 => '0', Key::Num1 => '1', Key::Num2 => '2', Key::Num3 => '3',
        Key::Num4 => '4', Key::Num5 => '5', Key::Num6 => '6', Key::Num7 => '7',
        Key::Num8 => '8', Key::Num9 => '9',
        _ => return None,
    };
    Some(c)
}
