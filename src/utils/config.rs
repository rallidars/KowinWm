use std::{
    fs,
    path::{Path, PathBuf},
};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smithay::input::keyboard::{keysyms::*, ModifiersState};

use crate::utils::action::{Action, Direction};

#[derive(Deserialize, Serialize)]
pub struct KeyboardConfig {
    pub layouts: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    pub workspaces: u8,
    pub border: Border,
    pub keyboard: KeyboardConfig,
    pub outputs: IndexMap<String, OutputData>,
    pub autostart: Vec<String>,
    pub keymaps: IndexMap<String, Action>,
}

impl Default for Config {
    fn default() -> Self {
        let workspaces = 4;
        let border = Border {
            thickness: 2,
            gap: 2,
            active: "#8B4000".to_string(),
            inactive: "#2A2A2A".to_string(),
        };
        let keyboard = KeyboardConfig {
            layouts: vec!["us".to_string()],
        };
        let mut outputs = IndexMap::new();
        outputs.insert(
            "DP-1".to_string(),
            OutputData {
                resolution: (2560, 1440),
                scale: 1.0,
                refresh_rate: 60,
                possition: (0, 0),
                enabled: true,
            },
        );
        let autostart = vec![];
        let mut keymaps = IndexMap::new();
        keymaps.insert("Super+c".to_string(), Action::KillActive);
        keymaps.insert("Super+Shift+Enter".to_string(), Action::Exit);
        keymaps.insert("Super+space".to_string(), Action::SwitchLayout);
        keymaps.insert("Super+f".to_string(), Action::Fullscreen);
        keymaps.insert("Super+r".to_string(), Action::ReloadConfig);
        for index in 1..5 {
            keymaps.insert(format!("Super+{index}"), Action::Workspace { index });
        }
        for index in 1..5 {
            keymaps.insert(
                format!("ctrl+alt+{index}"),
                Action::MoveToWorkspace { index },
            );
        }
        keymaps.insert(
            "Super+l".to_string(),
            Action::MoveFocus {
                direction: Direction::Right,
            },
        );
        keymaps.insert(
            "Super+h".to_string(),
            Action::MoveFocus {
                direction: Direction::Left,
            },
        );
        keymaps.insert(
            "Super+k".to_string(),
            Action::MoveFocus {
                direction: Direction::Top,
            },
        );
        keymaps.insert(
            "Super+j".to_string(),
            Action::MoveFocus {
                direction: Direction::Down,
            },
        );
        keymaps.insert(
            "ctrl+alt+l".to_string(),
            Action::MoveWindow {
                direction: Direction::Right,
            },
        );
        keymaps.insert(
            "ctrl+alt+h".to_string(),
            Action::MoveWindow {
                direction: Direction::Left,
            },
        );
        keymaps.insert(
            "ctrl+alt+k".to_string(),
            Action::MoveWindow {
                direction: Direction::Top,
            },
        );
        keymaps.insert(
            "ctrl+alt+j".to_string(),
            Action::MoveWindow {
                direction: Direction::Down,
            },
        );
        keymaps.insert(
            "Super+q".to_string(),
            Action::Exec {
                command: "kitty".to_string(),
            },
        );
        keymaps.insert(
            "Super+Tab".to_string(),
            Action::Exec {
                command: "rofi -show drun".to_string(),
            },
        );

        Self {
            workspaces,
            border,
            keyboard,
            outputs,
            autostart,
            keymaps,
        }
    }
}

impl Config {
    pub fn get_config() -> Option<Config> {
        let home_path = std::env::var("HOME").ok()?;
        let dir_path = format!("{home_path}/.config/kowinwm/");
        let mut config_path = PathBuf::new();
        config_path.push(&dir_path);
        config_path.push("config.toml");
        let data = if config_path.exists() {
            let file_data = std::fs::read_to_string(config_path).ok()?;
            toml::from_str(&file_data).ok()
        } else {
            let config = Config::default();
            let data = toml::to_string(&config).ok().unwrap();
            fs::create_dir_all(&dir_path).ok()?;
            fs::write(config_path, data).ok();
            Some(config)
        };
        data
    }
}

#[derive(Deserialize, Serialize)]
pub struct Border {
    pub thickness: i32,
    pub gap: i32,
    pub active: String,
    pub inactive: String,
}

#[derive(Deserialize, Serialize)]
pub struct OutputData {
    pub resolution: (i32, i32),
    pub refresh_rate: i32,
    pub scale: f64,
    pub possition: (i32, i32),
    pub enabled: bool,
}

pub fn parse_keybind(keybind: &str) -> Option<(ModifiersState, u32)> {
    let parts: Vec<&str> = keybind.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }

    let mut modifiers = ModifiersState::default();
    let key_part = parts.last().unwrap().to_lowercase();

    // Process all parts except the last as modifiers
    for &part in &parts[..parts.len() - 1] {
        match part.to_lowercase().as_str() {
            "super" | "logo" => modifiers.logo = true,
            "shift" => modifiers.shift = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" => modifiers.alt = true,
            _ => return None, // Unknown modifier
        }
    }

    // Now map the final part to a keysym
    let keysym = match key_part.as_str() {
        // Letters (a-z)
        "a" => KEY_a,
        "b" => KEY_b,
        "c" => KEY_c,
        "d" => KEY_d,
        "e" => KEY_e,
        "f" => KEY_f,
        "g" => KEY_g,
        "h" => KEY_h,
        "i" => KEY_i,
        "j" => KEY_j,
        "k" => KEY_k,
        "l" => KEY_l,
        "m" => KEY_m,
        "n" => KEY_n,
        "o" => KEY_o,
        "p" => KEY_p,
        "q" => KEY_q,
        "r" => KEY_r,
        "s" => KEY_s,
        "t" => KEY_t,
        "u" => KEY_u,
        "v" => KEY_v,
        "w" => KEY_w,
        "x" => KEY_x,
        "y" => KEY_y,
        "z" => KEY_z,

        // Letters (A-Z)
        "A" => KEY_A,
        "B" => KEY_B,
        "C" => KEY_C,
        "D" => KEY_D,
        "E" => KEY_E,
        "F" => KEY_F,
        "G" => KEY_G,
        "H" => KEY_H,
        "I" => KEY_I,
        "J" => KEY_J,
        "K" => KEY_K,
        "L" => KEY_L,
        "M" => KEY_M,
        "N" => KEY_N,
        "O" => KEY_O,
        "P" => KEY_P,
        "Q" => KEY_Q,
        "R" => KEY_R,
        "S" => KEY_S,
        "T" => KEY_T,
        "U" => KEY_U,
        "V" => KEY_V,
        "W" => KEY_W,
        "X" => KEY_X,
        "Y" => KEY_Y,
        "Z" => KEY_Z,

        // Numbers (top row)
        "0" => KEY_0,
        "1" => KEY_1,
        "2" => KEY_2,
        "3" => KEY_3,
        "4" => KEY_4,
        "5" => KEY_5,
        "6" => KEY_6,
        "7" => KEY_7,
        "8" => KEY_8,
        "9" => KEY_9,

        // Function keys
        "f1" => KEY_F1,
        "f2" => KEY_F2,
        "f3" => KEY_F3,
        "f4" => KEY_F4,
        "f5" => KEY_F5,
        "f6" => KEY_F6,
        "f7" => KEY_F7,
        "f8" => KEY_F8,
        "f9" => KEY_F9,
        "f10" => KEY_F10,
        "f11" => KEY_F11,
        "f12" => KEY_F12,
        "f13" => KEY_F13,
        "f14" => KEY_F14,
        "f15" => KEY_F15,
        "f16" => KEY_F16,
        "f17" => KEY_F17,
        "f18" => KEY_F18,
        "f19" => KEY_F19,
        "f20" => KEY_F20,
        "f21" => KEY_F21,
        "f22" => KEY_F22,
        "f23" => KEY_F23,
        "f24" => KEY_F24,

        // Arrows
        "up" | "uparrow" => KEY_Up,
        "down" | "downarrow" => KEY_Down,
        "left" | "leftarrow" => KEY_Left,
        "right" | "rightarrow" => KEY_Right,

        // Navigation / editing
        "escape" | "esc" => KEY_Escape,
        "tab" => KEY_Tab,
        "enter" | "return" => KEY_Return,
        "space" => KEY_space,
        "backspace" => KEY_BackSpace,
        "delete" | "del" => KEY_Delete,
        "insert" => KEY_Insert,
        "home" => KEY_Home,
        "end" => KEY_End,
        "pageup" | "pgup" => KEY_Page_Up,
        "pagedown" | "pgdown" => KEY_Page_Down,

        // Modifiers (sometimes people bind them directly)
        "shift" => KEY_Shift_L, // or Shift_R if you want right
        "ctrl" | "control" => KEY_Control_L,
        "alt" | "menu" => KEY_Alt_L,
        "super" | "logo" | "win" | "windows" => KEY_Super_L,

        // Common punctuation / symbols
        "minus" | "-" => KEY_minus,
        "equal" | "=" => KEY_equal,
        "bracketleft" | "[" => KEY_bracketleft,
        "bracketright" | "]" => KEY_bracketright,
        "semicolon" | ";" => KEY_semicolon,
        "apostrophe" | "'" => KEY_apostrophe,
        "grave" | "`" => KEY_grave,
        "backslash" | "\\" => KEY_backslash,
        "comma" | "," => KEY_comma,
        "period" | "." => KEY_period,
        "slash" | "/" => KEY_slash,

        // Numpad (useful for some bindings)
        "kp0" | "numpad0" => KEY_KP_0,
        "kp1" | "numpad1" => KEY_KP_1,
        "kp2" | "numpad2" => KEY_KP_2,
        "kp3" | "numpad3" => KEY_KP_3,
        "kp4" | "numpad4" => KEY_KP_4,
        "kp5" | "numpad5" => KEY_KP_5,
        "kp6" | "numpad6" => KEY_KP_6,
        "kp7" | "numpad7" => KEY_KP_7,
        "kp8" | "numpad8" => KEY_KP_8,
        "kp9" | "numpad9" => KEY_KP_9,
        "kpenter" => KEY_KP_Enter,
        "kpadd" | "kpplus" => KEY_KP_Add,
        "kpsub" | "kpminus" => KEY_KP_Subtract,
        "kpmul" | "kpmultiply" => KEY_KP_Multiply,
        "kpdiv" | "kpdivide" => KEY_KP_Divide,

        // Media / XF86 keys (very common for volume, brightness, etc.)
        "volumeup" | "audiovolup" => KEY_XF86AudioRaiseVolume,
        "volumedown" | "audiovoldown" => KEY_XF86AudioLowerVolume,
        "mute" | "audiomute" => KEY_XF86AudioMute,
        "play" | "audioplay" => KEY_XF86AudioPlay,
        "pause" | "audiopause" => KEY_XF86AudioPause,
        "next" | "audionext" => KEY_XF86AudioNext,
        "prev" | "audioprev" => KEY_XF86AudioPrev,
        "brightnessup" => KEY_XF86MonBrightnessUp,
        "brightnessdown" => KEY_XF86MonBrightnessDown,
        "print" | "printscreen" => KEY_Print,

        // Anything unknown
        _ => return None,
    };

    Some((modifiers, keysym))
}
