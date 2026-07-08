use std::collections::HashSet;

use enigo::{
    Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings,
};
use protocol::{KeyCode, MouseButton, PointerAction};

use crate::{InputError, InputInjector};

/// Input injection backed by the `enigo` crate.
///
/// Tracks currently-held keys and buttons so [`InputInjector::release_all`] can
/// clear them on disconnect.
pub struct EnigoInjector {
    enigo: Enigo,
    held_keys: HashSet<KeyCode>,
    held_buttons: HashSet<MouseButton>,
}

impl std::fmt::Debug for EnigoInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnigoInjector")
            .field("held_keys", &self.held_keys.len())
            .field("held_buttons", &self.held_buttons.len())
            .finish()
    }
}

impl EnigoInjector {
    pub fn new() -> Result<Self, InputError> {
        let enigo = Enigo::new(&Settings::default()).map_err(|e| InputError::Init(e.to_string()))?;
        Ok(Self {
            enigo,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
        })
    }
}

impl InputInjector for EnigoInjector {
    fn move_pointer(&mut self, x: i32, y: i32) -> Result<(), InputError> {
        self.enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| InputError::Inject(e.to_string()))
    }

    fn mouse_button(&mut self, button: MouseButton, action: PointerAction) -> Result<(), InputError> {
        let direction = to_direction(action);
        self.enigo
            .button(to_button(button), direction)
            .map_err(|e| InputError::Inject(e.to_string()))?;
        match action {
            PointerAction::Press => {
                self.held_buttons.insert(button);
            }
            PointerAction::Release => {
                self.held_buttons.remove(&button);
            }
        }
        Ok(())
    }

    fn mouse_wheel(&mut self, delta_x: f32, delta_y: f32) -> Result<(), InputError> {
        let steps_x = delta_x.round() as i32;
        let steps_y = delta_y.round() as i32;
        if steps_x != 0 {
            self.enigo
                .scroll(steps_x, Axis::Horizontal)
                .map_err(|e| InputError::Inject(e.to_string()))?;
        }
        if steps_y != 0 {
            // enigo scrolls positive = down; wire uses positive = up.
            self.enigo
                .scroll(-steps_y, Axis::Vertical)
                .map_err(|e| InputError::Inject(e.to_string()))?;
        }
        Ok(())
    }

    fn key(&mut self, key: KeyCode, action: PointerAction) -> Result<(), InputError> {
        let enigo_key = to_key(key)?;
        self.enigo
            .key(enigo_key, to_direction(action))
            .map_err(|e| InputError::Inject(e.to_string()))?;
        match action {
            PointerAction::Press => {
                self.held_keys.insert(key);
            }
            PointerAction::Release => {
                self.held_keys.remove(&key);
            }
        }
        Ok(())
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        let keys: Vec<KeyCode> = self.held_keys.drain().collect();
        let buttons: Vec<MouseButton> = self.held_buttons.drain().collect();
        for key in keys {
            if let Ok(enigo_key) = to_key(key) {
                let _ = self.enigo.key(enigo_key, Direction::Release);
            }
        }
        for button in buttons {
            let _ = self.enigo.button(to_button(button), Direction::Release);
        }
        Ok(())
    }
}

fn to_direction(action: PointerAction) -> Direction {
    match action {
        PointerAction::Press => Direction::Press,
        PointerAction::Release => Direction::Release,
    }
}

fn to_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn to_key(key: KeyCode) -> Result<Key, InputError> {
    let mapped = match key {
        KeyCode::Unicode(c) => Key::Unicode(c),
        KeyCode::Enter => Key::Return,
        KeyCode::Escape => Key::Escape,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Space => Key::Space,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::ArrowUp => Key::UpArrow,
        KeyCode::ArrowDown => Key::DownArrow,
        KeyCode::ArrowLeft => Key::LeftArrow,
        KeyCode::ArrowRight => Key::RightArrow,
        KeyCode::Shift => Key::Shift,
        KeyCode::Control => Key::Control,
        KeyCode::Alt => Key::Alt,
        KeyCode::Meta => Key::Meta,
        KeyCode::CapsLock => Key::CapsLock,
        KeyCode::Function(n) => function_key(n)?,
    };
    Ok(mapped)
}

fn function_key(n: u8) -> Result<Key, InputError> {
    let key = match n {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        other => return Err(InputError::Inject(format!("unsupported function key F{other}"))),
    };
    Ok(key)
}
