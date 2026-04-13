use std::{fs, path::PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use smithay::input::keyboard::{xkb, Keysym, ModifiersState};

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
            end_active: None,
            inactive: "#2A2A2A".to_string(),
            end_inactive: None,
            angle: None,
        };
        let keyboard = KeyboardConfig {
            layouts: vec!["us".to_string()],
        };
        let mut outputs = IndexMap::new();
        outputs.insert(
            "DP-1".to_string(),
            OutputData {
                resolution: Some((2560, 1440)),
                scale: Some(1.0),
                refresh_rate: Some(60),
                possition: Some((0, 0)),
                mirror: None,
                transform: None,
                workspaces: None,
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
    pub end_active: Option<String>,
    pub inactive: String,
    pub end_inactive: Option<String>,
    pub angle: Option<f32>,
}

#[derive(Deserialize, Serialize)]
pub struct OutputData {
    pub resolution: Option<(i32, i32)>,
    pub refresh_rate: Option<i32>,
    pub scale: Option<f64>,
    pub possition: Option<(i32, i32)>,
    pub mirror: Option<String>,
    pub transform: Option<String>,
    pub workspaces: Option<Vec<u8>>,
    pub enabled: bool,
}

pub fn parse_keybind(keybind: &str) -> Option<(ModifiersState, Keysym)> {
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

    // convert key name -> keysym
    let keysym = xkb::keysym_from_name(&key_part, xkb::KEYSYM_CASE_INSENSITIVE);

    Some((modifiers, keysym))
}
