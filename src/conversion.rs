use iced_core::keyboard;
use iced_core::mouse;
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};

pub(crate) fn modifiers_to_iced(mods: Modifiers) -> keyboard::Modifiers {
    let mut result = keyboard::Modifiers::empty();
    if mods.shift {
        result |= keyboard::Modifiers::SHIFT;
    }
    if mods.ctrl {
        result |= keyboard::Modifiers::CTRL;
    }
    if mods.alt {
        result |= keyboard::Modifiers::ALT;
    }
    if mods.logo {
        result |= keyboard::Modifiers::LOGO;
    }
    result
}

pub(crate) fn pointer_button_to_iced(button: u32) -> mouse::Button {
    match button {
        0x110 => mouse::Button::Left,   // BTN_LEFT
        0x111 => mouse::Button::Right,  // BTN_RIGHT
        0x112 => mouse::Button::Middle, // BTN_MIDDLE
        other => mouse::Button::Other(other as u16),
    }
}

/// Convert keysym to iced Key. This returns the BASE key (unmodified),
/// used for the `key` field in KeyPressed. The keysym represents the
/// logical key regardless of Ctrl/Alt modifiers.
pub(crate) fn keysym_to_iced_key(keysym: Keysym) -> keyboard::Key {
    if let Some(named) = keysym_to_named(keysym) {
        return keyboard::Key::Named(named);
    }
    // For ASCII-range keysyms, convert directly to character.
    // This ensures Ctrl+C produces Key::Character("c"), not Key::Character("\x03").
    let raw = keysym.raw();
    if (0x20..=0x7e).contains(&raw) {
        return keyboard::Key::Character(String::from(char::from(raw as u8)).into());
    }
    // For non-ASCII keysyms (e.g. accented characters), try xkb mapping
    if let Some(c) = keysym_to_char(raw) {
        return keyboard::Key::Character(String::from(c).into());
    }
    keyboard::Key::Unidentified
}

fn keysym_to_named(keysym: Keysym) -> Option<keyboard::key::Named> {
    use keyboard::key::Named;

    // XKB keysym constants
    Some(match keysym.raw() {
        // Function keys
        0xff08 => Named::Backspace,
        0xff09 => Named::Tab,
        0xff0d => Named::Enter,
        0xff1b => Named::Escape,
        0xffff => Named::Delete,

        // Navigation
        0xff50 => Named::Home,
        0xff51 => Named::ArrowLeft,
        0xff52 => Named::ArrowUp,
        0xff53 => Named::ArrowRight,
        0xff54 => Named::ArrowDown,
        0xff55 => Named::PageUp,
        0xff56 => Named::PageDown,
        0xff57 => Named::End,

        // Modifiers
        0xffe1 | 0xffe2 => Named::Shift,
        0xffe3 | 0xffe4 => Named::Control,
        0xffe5 => Named::CapsLock,
        0xffe7 | 0xffe8 => Named::Super,
        0xffe9 | 0xffea => Named::Alt,

        // Editing
        0xff63 => Named::Insert,

        // Whitespace
        0x0020 => Named::Space,

        // Function keys F1-F12
        0xffbe => Named::F1,
        0xffbf => Named::F2,
        0xffc0 => Named::F3,
        0xffc1 => Named::F4,
        0xffc2 => Named::F5,
        0xffc3 => Named::F6,
        0xffc4 => Named::F7,
        0xffc5 => Named::F8,
        0xffc6 => Named::F9,
        0xffc7 => Named::F10,
        0xffc8 => Named::F11,
        0xffc9 => Named::F12,

        // Media keys
        0x1008ff11 => Named::AudioVolumeDown,
        0x1008ff12 => Named::AudioVolumeMute,
        0x1008ff13 => Named::AudioVolumeUp,
        0x1008ff14 => Named::MediaPlayPause,
        0x1008ff15 => Named::MediaStop,
        0x1008ff16 => Named::MediaTrackPrevious,
        0x1008ff17 => Named::MediaTrackNext,

        // Misc
        0xff61 => Named::PrintScreen,
        0xff13 => Named::Pause,
        0xff14 => Named::ScrollLock,
        0xff67 => Named::ContextMenu,
        0xff7f => Named::NumLock,

        _ => return None,
    })
}

/// Convert XKB keysym to Unicode character (for non-ASCII keysyms).
fn keysym_to_char(keysym: u32) -> Option<char> {
    // Unicode keysyms: 0x01000000 + codepoint
    if keysym >= 0x01000000 {
        return char::from_u32(keysym - 0x01000000);
    }
    // Latin-1 supplement: keysyms 0xa0-0xff map to Unicode U+00A0-U+00FF
    if (0xa0..=0xff).contains(&keysym) {
        return char::from_u32(keysym);
    }
    None
}

/// Get the text that should be inserted for a key event.
/// Filters out control characters that shouldn't produce text input.
pub(crate) fn key_utf8_to_text(utf8: Option<&str>) -> Option<iced_core::SmolStr> {
    utf8.filter(|s| !s.is_empty() && s.chars().all(|c| !c.is_control()))
        .map(iced_core::SmolStr::from)
}

/// Map Linux evdev scancode to iced Physical key.
/// Mapping derived from linux/include/uapi/linux/input-event-codes.h,
/// matching winit's scancode_to_physicalkey().
pub(crate) fn scancode_to_physical(scancode: u32) -> keyboard::key::Physical {
    use keyboard::key::{Code, NativeCode, Physical};
    Physical::Code(match scancode {
        0 => return Physical::Unidentified(NativeCode::Xkb(0)),
        1 => Code::Escape,
        2 => Code::Digit1,
        3 => Code::Digit2,
        4 => Code::Digit3,
        5 => Code::Digit4,
        6 => Code::Digit5,
        7 => Code::Digit6,
        8 => Code::Digit7,
        9 => Code::Digit8,
        10 => Code::Digit9,
        11 => Code::Digit0,
        12 => Code::Minus,
        13 => Code::Equal,
        14 => Code::Backspace,
        15 => Code::Tab,
        16 => Code::KeyQ,
        17 => Code::KeyW,
        18 => Code::KeyE,
        19 => Code::KeyR,
        20 => Code::KeyT,
        21 => Code::KeyY,
        22 => Code::KeyU,
        23 => Code::KeyI,
        24 => Code::KeyO,
        25 => Code::KeyP,
        26 => Code::BracketLeft,
        27 => Code::BracketRight,
        28 => Code::Enter,
        29 => Code::ControlLeft,
        30 => Code::KeyA,
        31 => Code::KeyS,
        32 => Code::KeyD,
        33 => Code::KeyF,
        34 => Code::KeyG,
        35 => Code::KeyH,
        36 => Code::KeyJ,
        37 => Code::KeyK,
        38 => Code::KeyL,
        39 => Code::Semicolon,
        40 => Code::Quote,
        41 => Code::Backquote,
        42 => Code::ShiftLeft,
        43 => Code::Backslash,
        44 => Code::KeyZ,
        45 => Code::KeyX,
        46 => Code::KeyC,
        47 => Code::KeyV,
        48 => Code::KeyB,
        49 => Code::KeyN,
        50 => Code::KeyM,
        51 => Code::Comma,
        52 => Code::Period,
        53 => Code::Slash,
        54 => Code::ShiftRight,
        55 => Code::NumpadMultiply,
        56 => Code::AltLeft,
        57 => Code::Space,
        58 => Code::CapsLock,
        59 => Code::F1,
        60 => Code::F2,
        61 => Code::F3,
        62 => Code::F4,
        63 => Code::F5,
        64 => Code::F6,
        65 => Code::F7,
        66 => Code::F8,
        67 => Code::F9,
        68 => Code::F10,
        69 => Code::NumLock,
        70 => Code::ScrollLock,
        71 => Code::Numpad7,
        72 => Code::Numpad8,
        73 => Code::Numpad9,
        74 => Code::NumpadSubtract,
        75 => Code::Numpad4,
        76 => Code::Numpad5,
        77 => Code::Numpad6,
        78 => Code::NumpadAdd,
        79 => Code::Numpad1,
        80 => Code::Numpad2,
        81 => Code::Numpad3,
        82 => Code::Numpad0,
        83 => Code::NumpadDecimal,
        85 => Code::Lang5,
        86 => Code::IntlBackslash,
        87 => Code::F11,
        88 => Code::F12,
        89 => Code::IntlRo,
        90 => Code::Lang3,
        91 => Code::Lang4,
        92 => Code::Convert,
        93 => Code::KanaMode,
        94 => Code::NonConvert,
        96 => Code::NumpadEnter,
        97 => Code::ControlRight,
        98 => Code::NumpadDivide,
        99 => Code::PrintScreen,
        100 => Code::AltRight,
        102 => Code::Home,
        103 => Code::ArrowUp,
        104 => Code::PageUp,
        105 => Code::ArrowLeft,
        106 => Code::ArrowRight,
        107 => Code::End,
        108 => Code::ArrowDown,
        109 => Code::PageDown,
        110 => Code::Insert,
        111 => Code::Delete,
        113 => Code::AudioVolumeMute,
        114 => Code::AudioVolumeDown,
        115 => Code::AudioVolumeUp,
        117 => Code::NumpadEqual,
        119 => Code::Pause,
        121 => Code::NumpadComma,
        122 => Code::Lang1,
        123 => Code::Lang2,
        124 => Code::IntlYen,
        125 => Code::SuperLeft,
        126 => Code::SuperRight,
        127 => Code::ContextMenu,
        163 => Code::MediaTrackNext,
        164 => Code::MediaPlayPause,
        165 => Code::MediaTrackPrevious,
        166 => Code::MediaStop,
        183 => Code::F13,
        184 => Code::F14,
        185 => Code::F15,
        186 => Code::F16,
        187 => Code::F17,
        188 => Code::F18,
        189 => Code::F19,
        190 => Code::F20,
        191 => Code::F21,
        192 => Code::F22,
        193 => Code::F23,
        194 => Code::F24,
        _ => return Physical::Unidentified(NativeCode::Xkb(scancode)),
    })
}
