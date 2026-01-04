use std::process::Command;

use serde::{Deserialize, Serialize};
use smithay::backend::session::Session;
use smithay::input::keyboard::XkbConfig;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgShellHandler;

use crate::state::State;
use crate::utils::config::Config;
use crate::utils::workspaces::{self, is_fullscreen};

#[derive(PartialEq, Serialize, Deserialize, Clone)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Action {
    Exec { command: String },
    KillActive,
    Workspace { index: usize },
    MoveToWorkspace { index: usize },
    Exit,
    Fullscreen,
    MoveFocus { direction: Direction },
    MoveWindow { direction: Direction },
    VTSwitch(i32),
    SwitchLayout,
    ReloadConfig,
}

#[derive(PartialEq, Serialize, Deserialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Top,
    Down,
}

impl Action {
    pub fn execute(&self, state: &mut State) {
        match self {
            Action::VTSwitch(vt) => {
                if let Err(err) = state.backend_data.session.change_vt(*vt) {
                    tracing::error!("Error changing vt: {}", err)
                }
            }
            Action::Exit => {
                state.loop_signal.stop();
            }
            Action::Exec { command } => {
                tracing::debug!("Spawning '{command}'");
                Command::new("/bin/sh")
                    .arg("-c")
                    .arg(command)
                    .spawn()
                    .map_err(|e| tracing::info!("Failed to spawn '{command}': {e}"))
                    .ok();
            }
            Action::KillActive => {
                let active = match state.workspaces.get_active_window() {
                    Some(w) => w,
                    None => return,
                };
                if let Some(toplevel) = active.toplevel() {
                    toplevel.send_close();
                }
            }
            Action::ReloadConfig => state.config = Config::get_config().unwrap_or_default(),
            Action::SwitchLayout => {
                let keyboard = state.seat.get_keyboard().unwrap();
                let current_pos = state
                    .config
                    .keyboard
                    .layouts
                    .iter()
                    .position(|l| *l == state.current_layout)
                    .unwrap_or(0);
                let layout = state
                    .config
                    .keyboard
                    .layouts
                    .get(current_pos + 1)
                    .map_or("us".to_string(), |v| v.to_string());

                state.current_layout = layout.clone();
                let xkb_config = XkbConfig {
                    layout: &layout,
                    ..Default::default()
                };
                let _ = keyboard.set_xkb_config(state, xkb_config);
            }
            Action::Workspace { index } => {
                state
                    .workspaces
                    .set_active_workspace(*index, &mut state.space);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveToWorkspace { index } => {
                state.workspaces.move_window_to_ws(*index, &mut state.space);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveWindow { direction } => {
                state
                    .workspaces
                    .move_window(direction, &mut state.pointer_location, &state.space);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveFocus { direction } => {
                state
                    .workspaces
                    .change_focus(direction, &mut state.pointer_location, &state.space);
                state.set_keyboard_focus_auto();
            }
            Action::Fullscreen => {
                let acitve_window = match &state.workspaces.get_current().active_window {
                    Some(active) => active,
                    None => return,
                };
                let elements = state.workspaces.get_current().layout.iter();
                if let Some(fullscreen) = is_fullscreen(elements) {
                    //if fullscreen == acitve_window {
                    //    state.unfullscreen_request(acitve_window.toplevel().unwrap().clone());
                    //}
                    state.unfullscreen_request(fullscreen.toplevel().unwrap().clone());
                } else {
                    state.fullscreen_request(acitve_window.toplevel().unwrap().clone(), None);
                }
            }
        };
    }
}
