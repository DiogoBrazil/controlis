use protocol::KeyCode;

/// Maps a protocol key to an X11 keysym for the RemoteDesktop portal.
///
/// Printable characters map directly (ASCII == keysym; other Unicode uses the
/// `0x01000000 + codepoint` XKB convention). Named keys use their XKB keysyms.
/// Keysym support varies by compositor (GNOME/KDE support it; some wlroots setups
/// prefer keycodes) — a known limitation documented for the Wayland phase.
pub fn to_keysym(key: KeyCode) -> Option<i32> {
    let sym: u32 = match key {
        KeyCode::Unicode(c) => unicode_keysym(c),
        KeyCode::Enter => 0xFF0D,
        KeyCode::Escape => 0xFF1B,
        KeyCode::Backspace => 0xFF08,
        KeyCode::Tab => 0xFF09,
        KeyCode::Space => 0x0020,
        KeyCode::Delete => 0xFFFF,
        KeyCode::Home => 0xFF50,
        KeyCode::End => 0xFF57,
        KeyCode::PageUp => 0xFF55,
        KeyCode::PageDown => 0xFF56,
        KeyCode::ArrowLeft => 0xFF51,
        KeyCode::ArrowUp => 0xFF52,
        KeyCode::ArrowRight => 0xFF53,
        KeyCode::ArrowDown => 0xFF54,
        KeyCode::Shift => 0xFFE1,
        KeyCode::Control => 0xFFE3,
        KeyCode::Alt => 0xFFE9,
        KeyCode::Meta => 0xFFEB,
        KeyCode::CapsLock => 0xFFE5,
        KeyCode::Function(n) if (1..=12).contains(&n) => 0xFFBE + (n as u32 - 1),
        KeyCode::Function(_) => return None,
    };
    i32::try_from(sym).ok()
}

fn unicode_keysym(c: char) -> u32 {
    let cp = c as u32;
    if cp <= 0x7F {
        cp
    } else {
        0x0100_0000 + cp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_maps_directly() {
        assert_eq!(to_keysym(KeyCode::Unicode('a')), Some(0x61));
        assert_eq!(to_keysym(KeyCode::Space), Some(0x20));
    }

    #[test]
    fn accented_uses_unicode_plane() {
        assert_eq!(to_keysym(KeyCode::Unicode('ç')), Some(0x0100_0000 + 0xE7));
    }

    #[test]
    fn named_keys_have_keysyms() {
        assert_eq!(to_keysym(KeyCode::Enter), Some(0xFF0D));
        assert_eq!(to_keysym(KeyCode::Function(1)), Some(0xFFBE));
        assert_eq!(to_keysym(KeyCode::Function(12)), Some(0xFFC9));
        assert_eq!(to_keysym(KeyCode::Function(20)), None);
    }
}
