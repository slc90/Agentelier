use super::{DesktopInput, after_cleanup, send_input};
use crate::ToolFailure;
use std::time::Duration;
use winsafe::{HwKbMouse, KEYBDINPUT, co};

const KEY_EVENT_GAP: Duration = Duration::from_millis(10);
const LETTERS: [co::VK; 26] = [
    co::VK::CHAR_A,
    co::VK::CHAR_B,
    co::VK::CHAR_C,
    co::VK::CHAR_D,
    co::VK::CHAR_E,
    co::VK::CHAR_F,
    co::VK::CHAR_G,
    co::VK::CHAR_H,
    co::VK::CHAR_I,
    co::VK::CHAR_J,
    co::VK::CHAR_K,
    co::VK::CHAR_L,
    co::VK::CHAR_M,
    co::VK::CHAR_N,
    co::VK::CHAR_O,
    co::VK::CHAR_P,
    co::VK::CHAR_Q,
    co::VK::CHAR_R,
    co::VK::CHAR_S,
    co::VK::CHAR_T,
    co::VK::CHAR_U,
    co::VK::CHAR_V,
    co::VK::CHAR_W,
    co::VK::CHAR_X,
    co::VK::CHAR_Y,
    co::VK::CHAR_Z,
];
const DIGITS: [co::VK; 10] = [
    co::VK::CHAR_0,
    co::VK::CHAR_1,
    co::VK::CHAR_2,
    co::VK::CHAR_3,
    co::VK::CHAR_4,
    co::VK::CHAR_5,
    co::VK::CHAR_6,
    co::VK::CHAR_7,
    co::VK::CHAR_8,
    co::VK::CHAR_9,
];
const FUNCTIONS: [co::VK; 24] = [
    co::VK::F1,
    co::VK::F2,
    co::VK::F3,
    co::VK::F4,
    co::VK::F5,
    co::VK::F6,
    co::VK::F7,
    co::VK::F8,
    co::VK::F9,
    co::VK::F10,
    co::VK::F11,
    co::VK::F12,
    co::VK::F13,
    co::VK::F14,
    co::VK::F15,
    co::VK::F16,
    co::VK::F17,
    co::VK::F18,
    co::VK::F19,
    co::VK::F20,
    co::VK::F21,
    co::VK::F22,
    co::VK::F23,
    co::VK::F24,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key(co::VK);

impl Key {
    pub fn from_name(name: &str) -> Result<Self, ToolFailure> {
        let name = name.trim().to_ascii_uppercase();
        let key = match name.as_bytes() {
            [letter @ b'A'..=b'Z'] => LETTERS.get(usize::from(*letter - b'A')).copied(),
            [digit @ b'0'..=b'9'] => DIGITS.get(usize::from(*digit - b'0')).copied(),
            _ => named_key(&name),
        };
        key.map(Self)
            .ok_or_else(|| ToolFailure::new("invalid_key", format!("不支持的键名 {name:?}")))
    }
}

fn named_key(name: &str) -> Option<co::VK> {
    Some(match name {
        "CTRL" | "CONTROL" | "LEFTCTRL" => co::VK::LCONTROL,
        "RIGHTCTRL" => co::VK::RCONTROL,
        "SHIFT" | "LEFTSHIFT" => co::VK::LSHIFT,
        "RIGHTSHIFT" => co::VK::RSHIFT,
        "ALT" | "LEFTALT" => co::VK::LMENU,
        "RIGHTALT" => co::VK::RMENU,
        "WIN" | "LEFTWIN" => co::VK::LWIN,
        "RIGHTWIN" => co::VK::RWIN,
        "ENTER" => co::VK::RETURN,
        "ESC" | "ESCAPE" => co::VK::ESCAPE,
        "SPACE" => co::VK::SPACE,
        "TAB" => co::VK::TAB,
        "BACKSPACE" => co::VK::BACK,
        "INSERT" => co::VK::INSERT,
        "DELETE" => co::VK::DELETE,
        "HOME" => co::VK::HOME,
        "END" => co::VK::END,
        "PAGEUP" => co::VK::PRIOR,
        "PAGEDOWN" => co::VK::NEXT,
        "UP" => co::VK::UP,
        "DOWN" => co::VK::DOWN,
        "LEFT" => co::VK::LEFT,
        "RIGHT" => co::VK::RIGHT,
        "PLUS" => co::VK::OEM_PLUS,
        "MINUS" => co::VK::OEM_MINUS,
        "IMEON" => co::VK::IME_ON,
        "IMEOFF" => co::VK::IME_OFF,
        _ => {
            let number = name.strip_prefix('F')?.parse::<usize>().ok()?;
            *FUNCTIONS.get(number.checked_sub(1)?)?
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeldKey {
    Virtual(Key),
    Unicode(u16),
}

impl HeldKey {
    fn send(self, released: bool) -> Result<(), ToolFailure> {
        let mut event = match self {
            Self::Virtual(Key(key)) => {
                let extended = matches!(
                    key,
                    co::VK::RCONTROL
                        | co::VK::RMENU
                        | co::VK::LWIN
                        | co::VK::RWIN
                        | co::VK::INSERT
                        | co::VK::DELETE
                        | co::VK::HOME
                        | co::VK::END
                        | co::VK::PRIOR
                        | co::VK::NEXT
                        | co::VK::UP
                        | co::VK::DOWN
                        | co::VK::LEFT
                        | co::VK::RIGHT
                );
                KEYBDINPUT {
                    wVk: key,
                    dwFlags: if extended {
                        co::KEYEVENTF::EXTENDEDKEY
                    } else {
                        co::KEYEVENTF::NoValue
                    },
                    ..KEYBDINPUT::default()
                }
            }
            Self::Unicode(unit) => KEYBDINPUT {
                wScan: unit,
                dwFlags: co::KEYEVENTF::UNICODE,
                ..KEYBDINPUT::default()
            },
        };
        if released {
            event.dwFlags |= co::KEYEVENTF::KEYUP;
        }
        send_input(HwKbMouse::Kb(event))
    }
}

impl DesktopInput {
    pub fn key_down(&mut self, key: Key) -> Result<(), ToolFailure> {
        self.check_cancel()?;
        let held = HeldKey::Virtual(key);
        if self.keys.contains(&held) {
            return Ok(());
        }
        if winsafe::GetAsyncKeyState(key.0) {
            return Err(ToolFailure::new(
                "key_in_use",
                format!("键 {key:?} 已被其他输入按住"),
            ));
        }
        held.send(false)?;
        self.keys.push(held);
        Ok(())
    }

    pub fn key_up(&mut self, key: Key) -> Result<(), ToolFailure> {
        if let Some(index) = self
            .keys
            .iter()
            .position(|held| *held == HeldKey::Virtual(key))
        {
            self.release_key(index)?;
        }
        Ok(())
    }

    pub fn press(&mut self, key: Key) -> Result<(), ToolFailure> {
        self.shortcut(&[key])
    }

    pub fn shortcut(&mut self, keys: &[Key]) -> Result<(), ToolFailure> {
        if keys.is_empty() {
            return Err(ToolFailure::new(
                "invalid_shortcut",
                "组合键必须包含至少一个键",
            ));
        }
        let start = self.keys.len();
        let result = keys.iter().try_for_each(|key| {
            if self.keys.contains(&HeldKey::Virtual(*key)) {
                return Err(ToolFailure::new(
                    "key_already_held",
                    format!("键 {key:?} 重复或已由此前的操作按下"),
                ));
            }
            self.key_down(*key)?;
            self.wait(KEY_EVENT_GAP)
        });
        after_cleanup(result, self.release_keys(start))
    }

    pub fn type_text(&mut self, text: &str) -> Result<(), ToolFailure> {
        self.check_cancel()?;
        for character in text.chars() {
            self.check_cancel()?;
            let mut buffer = [0; 2];
            for unit in character.encode_utf16(&mut buffer) {
                let held = HeldKey::Unicode(*unit);
                held.send(false)?;
                let index = self.keys.len();
                self.keys.push(held);
                self.release_key(index)?;
            }
        }
        Ok(())
    }

    pub(super) fn release_keys(&mut self, start: usize) -> Result<(), ToolFailure> {
        let errors: Vec<_> = (start..self.keys.len())
            .rev()
            .filter_map(|index| self.release_key(index).err())
            .map(|error| error.to_string())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ToolFailure::new(
                "desktop_input_cleanup",
                errors.join(" | "),
            ))
        }
    }

    fn release_key(&mut self, index: usize) -> Result<(), ToolFailure> {
        self.keys[index].send(true)?;
        self.keys.remove(index);
        Ok(())
    }
}
