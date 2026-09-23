//! Configured Win32 keyboard chords. Modifiers match exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: usize,
    pub modifiers: u8,
}
impl Chord {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        let mut modifiers = 0;
        let mut key = None;
        for part in text.split('+') {
            let token = part.trim().to_ascii_uppercase();
            let modifier = match token.as_str() {
                "CTRL" => 1,
                "ALT" => 2,
                "SHIFT" => 4,
                _ => 0,
            };
            if modifier != 0 {
                if modifiers & modifier != 0 {
                    return Err("duplicate modifier");
                }
                modifiers |= modifier;
            } else {
                if key.is_some() {
                    return Err("use exactly one key");
                }
                key = Some(key_code(&token).ok_or("unsupported key; see regroup README")?);
            }
        }
        Ok(Self {
            key: key.ok_or("missing key")?,
            modifiers,
        })
    }
    pub fn matches(self, message: u32, key: usize, flags: usize, modifiers: u8) -> bool {
        matches!(message, 0x100 | 0x104)
            && flags & (1 << 30) == 0
            && self.key == key
            && self.modifiers == modifiers
    }
}
fn key_code(key: &str) -> Option<usize> {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(key.as_bytes()[0] as usize);
    }
    if let Some(n) = key.strip_prefix('F').and_then(|s| s.parse::<usize>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x6f + n);
        }
    }
    if let Some(n) = key
        .strip_prefix("NUMPAD")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    Some(match key {
        "SPACE" => 0x20,
        "TAB" => 9,
        "ENTER" => 13,
        "ESCAPE" => 27,
        "BACKSPACE" => 8,
        "INSERT" => 0x2d,
        "DELETE" => 0x2e,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "MULTIPLY" => 0x6a,
        "ADD" => 0x6b,
        "SUBTRACT" => 0x6d,
        "DECIMAL" => 0x6e,
        "DIVIDE" => 0x6f,
        _ => return None,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_and_match() {
        let c = Chord::parse(" shift + Ctrl + F12 ").unwrap();
        assert_eq!(
            c,
            Chord {
                key: 0x7b,
                modifiers: 5
            }
        );
        assert!(c.matches(0x100, 0x7b, 0, 5));
        assert!(c.matches(0x104, 0x7b, 1 << 29, 5));
        for (msg, key, flags, mods) in [
            (0x101, 0x7b, 0, 5),
            (0x100, 0x7b, 1 << 30, 5),
            (0x100, 0x7b, 0, 7),
            (0x100, 0x7a, 0, 5),
        ] {
            assert!(!c.matches(msg, key, flags, mods));
        }
        for bad in [
            "",
            "Ctrl",
            "Ctrl+Ctrl+R",
            "R+U",
            "Win+R",
            "F25",
            "Numpad10",
            "Alt+",
        ] {
            assert!(Chord::parse(bad).is_err(), "{bad}");
        }
        for good in [
            "R",
            "Ctrl+Alt+U",
            "0",
            "F1",
            "F24",
            "Numpad0",
            "Numpad9",
            "Shift+PageDown",
            "Alt+Divide",
        ] {
            assert!(Chord::parse(good).is_ok(), "{good}");
        }
    }
}
