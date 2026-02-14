use std::cell::RefCell;
use std::process::Command;

use serde::{Deserialize, Serialize};
use smithay::backend::session::Session;
use smithay::desktop::WindowSurface;
use smithay::input::keyboard::XkbConfig;
use smithay::input::pointer::{Focus, GrabStartData};
use smithay::utils::{Logical, Point};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgShellHandler;
#[cfg(feature = "xwayland")]
use smithay::xwayland::XwmHandler;

use crate::state::State;
use crate::utils::config::Config;
use crate::utils::grab::MovePointerGrab;
use crate::utils::workspaces::{is_fullscreen, place_on_center, WindowMode, WindowUserData};
use crate::SERIAL_COUNTER;

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
    FloatingWindow,
    MoveWindowMouse,
    ResizeWindowMouse,
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
        let pointer = state.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        if pointer.is_grabbed() {
            pointer.unset_grab(state, serial, 0);
            match self {
                Action::MoveWindowMouse => return,
                _ => {}
            }
        }
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
                    .env(
                        "WAYLAND_DISPLAY",
                        state.socket_name.to_string_lossy().into_owned().clone(),
                    )
                    .spawn()
                    .map_err(|e| tracing::info!("Failed to spawn '{command}': {e}"))
                    .ok();
            }
            Action::KillActive => {
                let active = match state.workspaces.get_active_window() {
                    Some(w) => w,
                    None => return,
                };
                match active.underlying_surface() {
                    WindowSurface::Wayland(xdg) => {
                        xdg.send_close();
                    }
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x11) => {
                        x11.close();
                    }
                }
            }
            Action::FloatingWindow => {
                let active = match state.workspaces.get_active_window() {
                    Some(w) => w,
                    None => return,
                };

                let mut user_data = active
                    .user_data()
                    .get::<RefCell<WindowUserData>>()
                    .unwrap()
                    .borrow_mut();
                let ws = state.workspaces.get_current_mut();
                match user_data.mode {
                    WindowMode::Tiled => {
                        user_data.mode = WindowMode::Floating;
                        place_on_center(&mut ws.space, &active);
                    }
                    WindowMode::Floating => {
                        user_data.mode = WindowMode::Tiled;
                    }
                    WindowMode::Fullscreen(_) => {}
                }
                drop(user_data);
                state.refresh_layout();
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
                state.workspaces.set_active_workspace(*index - 1);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveToWorkspace { index } => {
                state.workspaces.move_window_to_ws(*index - 1);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveWindow { direction } => {
                state
                    .workspaces
                    .move_window(direction, &mut state.pointer_location);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveFocus { direction } => {
                state
                    .workspaces
                    .change_focus(direction, &mut state.pointer_location);
                state.set_keyboard_focus_auto();
            }
            Action::Fullscreen => {
                let active_window = match &state.workspaces.get_current().active_window {
                    Some(active) => active,
                    None => return,
                };
                let elements = state.workspaces.get_current().space.elements();
                if let Some(fullscreen) = is_fullscreen(elements) {
                    //if fullscreen == acitve_window {
                    //    state.unfullscreen_request(acitve_window.toplevel().unwrap().clone());
                    //}
                    match fullscreen.underlying_surface() {
                        WindowSurface::Wayland(xdg) => {
                            XdgShellHandler::unfullscreen_request(state, xdg.clone());
                        }
                        #[cfg(feature = "xwayland")]
                        WindowSurface::X11(x11) => {
                            let xwm_id = state.xwm.as_ref().unwrap().id();
                            XwmHandler::unfullscreen_request(state, xwm_id, x11.clone());
                        }
                    }
                } else {
                    match active_window.underlying_surface() {
                        WindowSurface::Wayland(xdg) => {
                            XdgShellHandler::fullscreen_request(state, xdg.clone(), None);
                        }
                        #[cfg(feature = "xwayland")]
                        WindowSurface::X11(x11) => {
                            let xwm_id = state.xwm.as_ref().unwrap().id();
                            XwmHandler::fullscreen_request(state, xwm_id, x11.clone());
                        }
                    }
                }
            }
            Action::MoveWindowMouse => {
                let surface = match state.surface_under() {
                    Some(surface) => surface,
                    None => return,
                };
                let ws = state.workspaces.get_current_mut();
                let window = ws
                    .space
                    .elements()
                    .find(|element| {
                        element
                            .wl_surface()
                            .map(|s| &*s == &surface.0)
                            .unwrap_or(false)
                    })
                    .unwrap()
                    .clone();
                tracing::info!("start reposition");
                let start_data = GrabStartData {
                    focus: Some(surface),
                    button: 272,
                    location: state.pointer_location,
                };
                let window_geo = match ws.space.element_geometry(&window) {
                    Some(l) => l,
                    None => return,
                };

                let pointer_pos = start_data.location;

                let start_loc: Point<i32, Logical> = (
                    pointer_pos.x as i32 - (window_geo.size.w as i32 / 2),
                    pointer_pos.y as i32 - (window_geo.size.h as i32 / 2),
                )
                    .into();

                window
                    .user_data()
                    .get::<RefCell<WindowUserData>>()
                    .unwrap()
                    .borrow_mut()
                    .mode = WindowMode::Floating;

                ws.space.map_element(window.clone(), start_loc, false);
                let grab = MovePointerGrab {
                    start_data,
                    window,
                    start_loc,
                };

                pointer.set_grab(state, grab, serial, Focus::Clear);
                state.refresh_layout();
            }
            Action::ResizeWindowMouse => {}
        };
    }
}
