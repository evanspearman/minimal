//! Keys from the window, as the X11 keysyms RFB carries.
//!
//! wayvnc turns each keysym back into a key on the box's keymap, so the
//! client sends what the key means, not where it is. Cmd on a Mac is
//! `NamedKey::Super` in winit and goes out as Super_L, the key a Wayland
//! compositor such as niri binds its shortcuts to.

use std::collections::HashMap;

use winit::keyboard::{Key, KeyLocation, NamedKey, PhysicalKey};

use super::rfb::Outbound;

/// The keysym for `key` at `location`, or `None` for a key RFB has no
/// keysym for (a dead key, a media key, a string of several characters).
pub(crate) fn keysym(key: &Key, location: KeyLocation) -> Option<u32> {
    match key {
        Key::Named(named) => named_keysym(*named, location),
        Key::Character(text) => {
            let mut chars = text.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Some(char_keysym(c))
        }
        Key::Dead(_) | Key::Unidentified(_) => None,
    }
}

/// Latin-1 characters are their own keysyms; everything else in Unicode is
/// `0x01000000` plus its code point.
fn char_keysym(c: char) -> u32 {
    let cp = u32::from(c);
    if (0x20..=0x7e).contains(&cp) || (0xa0..=0xff).contains(&cp) {
        cp
    } else {
        0x0100_0000 | cp
    }
}

fn named_keysym(key: NamedKey, location: KeyLocation) -> Option<u32> {
    let right = location == KeyLocation::Right;
    let numpad = location == KeyLocation::Numpad;
    let keysym = match key {
        NamedKey::Space => 0x0020,
        NamedKey::Backspace => 0xff08,
        NamedKey::Tab => 0xff09,
        NamedKey::Enter if numpad => 0xff8d,
        NamedKey::Enter => 0xff0d,
        NamedKey::Pause => 0xff13,
        NamedKey::ScrollLock => 0xff14,
        NamedKey::Escape => 0xff1b,
        NamedKey::Home => 0xff50,
        NamedKey::ArrowLeft => 0xff51,
        NamedKey::ArrowUp => 0xff52,
        NamedKey::ArrowRight => 0xff53,
        NamedKey::ArrowDown => 0xff54,
        NamedKey::PageUp => 0xff55,
        NamedKey::PageDown => 0xff56,
        NamedKey::End => 0xff57,
        NamedKey::PrintScreen => 0xff61,
        NamedKey::Insert => 0xff63,
        NamedKey::ContextMenu => 0xff67,
        NamedKey::NumLock => 0xff7f,
        NamedKey::Delete => 0xffff,
        NamedKey::Shift if right => 0xffe2,
        NamedKey::Shift => 0xffe1,
        NamedKey::Control if right => 0xffe4,
        NamedKey::Control => 0xffe3,
        NamedKey::CapsLock => 0xffe5,
        NamedKey::Alt if right => 0xffea,
        NamedKey::Alt => 0xffe9,
        NamedKey::AltGraph => 0xfe03,
        NamedKey::Super | NamedKey::Meta if right => 0xffec,
        NamedKey::Super | NamedKey::Meta => 0xffeb,
        NamedKey::F1 => 0xffbe,
        NamedKey::F2 => 0xffbf,
        NamedKey::F3 => 0xffc0,
        NamedKey::F4 => 0xffc1,
        NamedKey::F5 => 0xffc2,
        NamedKey::F6 => 0xffc3,
        NamedKey::F7 => 0xffc4,
        NamedKey::F8 => 0xffc5,
        NamedKey::F9 => 0xffc6,
        NamedKey::F10 => 0xffc7,
        NamedKey::F11 => 0xffc8,
        NamedKey::F12 => 0xffc9,
        _ => return None,
    };
    Some(keysym)
}

/// The keys held down, so each release sends the keysym its press sent.
///
/// A key's meaning can change while it is held (Shift goes down, a layout
/// switches), and the server only releases what it was told was pressed.
#[derive(Debug, Default)]
pub(crate) struct KeyTracker {
    held: HashMap<PhysicalKey, u32>,
}

impl KeyTracker {
    /// The message for a press, or `None` for an auto-repeat (the box's
    /// compositor repeats keys itself) or a key with no keysym.
    pub(crate) fn press(&mut self, physical: PhysicalKey, keysym: Option<u32>) -> Option<Outbound> {
        if self.held.contains_key(&physical) {
            return None;
        }
        let keysym = keysym?;
        self.held.insert(physical, keysym);
        Some(Outbound::Key { down: true, keysym })
    }

    /// The message for a release of a key this tracker saw pressed.
    pub(crate) fn release(&mut self, physical: PhysicalKey) -> Option<Outbound> {
        let keysym = self.held.remove(&physical)?;
        Some(Outbound::Key {
            down: false,
            keysym,
        })
    }

    /// Releases for every key still held: the window lost focus, and the
    /// releases will go to another window.
    pub(crate) fn release_all(&mut self) -> Vec<Outbound> {
        self.held
            .drain()
            .map(|(_, keysym)| Outbound::Key {
                down: false,
                keysym,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

    use super::*;

    fn named(key: NamedKey, location: KeyLocation) -> Option<u32> {
        keysym(&Key::Named(key), location)
    }

    fn character(text: &str) -> Option<u32> {
        keysym(&Key::Character(text.into()), KeyLocation::Standard)
    }

    #[test]
    fn cmd_is_super_l_and_its_right_hand_twin_super_r() {
        assert_eq!(named(NamedKey::Super, KeyLocation::Left), Some(0xffeb));
        assert_eq!(named(NamedKey::Super, KeyLocation::Right), Some(0xffec));
    }

    #[test]
    fn modifiers_keep_their_side() {
        for (key, left, right) in [
            (NamedKey::Shift, 0xffe1, 0xffe2),
            (NamedKey::Control, 0xffe3, 0xffe4),
            (NamedKey::Alt, 0xffe9, 0xffea),
        ] {
            assert_eq!(named(key, KeyLocation::Left), Some(left));
            assert_eq!(named(key, KeyLocation::Right), Some(right));
        }
    }

    #[test]
    fn editing_and_navigation_keys_have_their_keysyms() {
        for (key, expected) in [
            (NamedKey::Enter, 0xff0d),
            (NamedKey::Escape, 0xff1b),
            (NamedKey::Backspace, 0xff08),
            (NamedKey::Tab, 0xff09),
            (NamedKey::Space, 0x20),
            (NamedKey::ArrowLeft, 0xff51),
            (NamedKey::F1, 0xffbe),
            (NamedKey::F12, 0xffc9),
        ] {
            assert_eq!(named(key, KeyLocation::Standard), Some(expected), "{key:?}");
        }
        assert_eq!(named(NamedKey::Enter, KeyLocation::Numpad), Some(0xff8d));
    }

    #[test]
    fn latin_1_characters_are_their_own_keysyms() {
        assert_eq!(character("a"), Some(0x61));
        assert_eq!(character("A"), Some(0x41));
        assert_eq!(character("~"), Some(0x7e));
        assert_eq!(character("é"), Some(0xe9));
    }

    #[test]
    fn other_characters_are_unicode_keysyms() {
        assert_eq!(character("€"), Some(0x0100_20ac));
        assert_eq!(character("ж"), Some(0x0100_0436));
    }

    #[test]
    fn keys_with_no_keysym_send_nothing() {
        assert_eq!(character("ab"), None);
        assert_eq!(keysym(&Key::Dead(Some('`')), KeyLocation::Standard), None);
        assert_eq!(named(NamedKey::MediaPlayPause, KeyLocation::Standard), None);
    }

    #[test]
    fn a_release_sends_the_keysym_its_press_sent() {
        let mut keys = KeyTracker::default();
        let a = PhysicalKey::Code(KeyCode::KeyA);
        assert_eq!(
            keys.press(a, Some(0x61)),
            Some(Outbound::Key {
                down: true,
                keysym: 0x61
            })
        );
        // Shift went down meanwhile: the key now means "A", but the release
        // is for the "a" the server saw pressed.
        assert_eq!(
            keys.release(a),
            Some(Outbound::Key {
                down: false,
                keysym: 0x61
            })
        );
        assert_eq!(keys.release(a), None, "released once");
    }

    #[test]
    fn a_held_key_is_pressed_once() {
        let mut keys = KeyTracker::default();
        let a = PhysicalKey::Code(KeyCode::KeyA);
        assert!(keys.press(a, Some(0x61)).is_some());
        assert_eq!(keys.press(a, Some(0x61)), None);
    }

    #[test]
    fn losing_focus_releases_every_held_key() {
        let mut keys = KeyTracker::default();
        keys.press(PhysicalKey::Code(KeyCode::SuperLeft), Some(0xffeb));
        keys.press(PhysicalKey::Code(KeyCode::KeyQ), Some(0x71));
        let mut released = keys.release_all();
        released.sort_by_key(|m| match m {
            Outbound::Key { keysym, .. } => *keysym,
            _ => 0,
        });
        assert_eq!(
            released,
            [
                Outbound::Key {
                    down: false,
                    keysym: 0x71
                },
                Outbound::Key {
                    down: false,
                    keysym: 0xffeb
                },
            ]
        );
        assert!(keys.release_all().is_empty());
    }
}
