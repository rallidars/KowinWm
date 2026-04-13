use std::cell::RefCell;
use std::process::Command;

use serde::{Deserialize, Serialize};
use smithay::backend::session::Session;
use smithay::desktop::WindowSurface;
use smithay::wayland::shell::xdg::XdgShellHandler;
#[cfg(feature = "xwayland")]
use smithay::xwayland::XwmHandler;

use crate::state::State;
use crate::utils::config::Config;
use crate::utils::workspaces::{is_fullscreen, place_on_center, WindowMode};
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
    ToggleLayout,
    PrevWorkspace,
    NextWorkspace,
    ResizeActive { direction: Direction, step: i32 },
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
                Action::MoveWindowMouse | Action::ResizeWindowMouse => return,
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
            Action::ToggleLayout => {
                let ws = state.workspaces.get_current_mut();
                match ws.layout {
                    super::layout::LayoutState::Floating => {
                        ws.layout = super::layout::LayoutState::default();
                        for item in ws.space.elements() {
                            if let Some(data) = item.user_data().get::<RefCell<WindowMode>>() {
                                *data.borrow_mut() = WindowMode::Tiled;
                            }
                        }
                    }
                    _ => {
                        ws.layout = super::layout::LayoutState::Floating;
                        for item in ws.space.elements() {
                            if let Some(data) = item.user_data().get::<RefCell<WindowMode>>() {
                                *data.borrow_mut() = WindowMode::Floating;
                            }
                        }
                    }
                }
                state.refresh_layout();
            }
            Action::KillActive => {
                let ws = state.workspaces.get_current();
                let active = match ws.get_active_window() {
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
                let ws = state.workspaces.get_current_mut();
                let active = match ws.get_active_window() {
                    Some(w) => w,
                    None => return,
                };

                let mut user_data = active
                    .user_data()
                    .get::<RefCell<WindowMode>>()
                    .unwrap()
                    .borrow_mut();
                match *user_data {
                    WindowMode::Tiled => {
                        *user_data = WindowMode::Floating;
                        place_on_center(
                            &mut ws.space,
                            &active,
                            state.config.border.gap + state.config.border.thickness,
                        );
                    }
                    WindowMode::Floating => {
                        *user_data = WindowMode::Tiled;
                    }
                    _ => {}
                }
                drop(user_data);
                state.refresh_layout();
            }
            Action::ReloadConfig => state.config = Config::get_config().unwrap_or_default(),
            Action::SwitchLayout => {
                let keyboard = state.seat.get_keyboard().unwrap();
                keyboard.with_xkb_state(state, |mut data| {
                    data.cycle_next_layout();
                });
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
                let ws = state.workspaces.get_current_mut();
                ws.move_window(direction, &mut state.pointer_location);
                state.refresh_layout();
                state.set_keyboard_focus_auto();
            }
            Action::MoveFocus { direction } => {
                let ws = state.workspaces.get_current_mut();
                ws.change_focus(direction, &mut state.pointer_location);
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
                            if let Some(xwm) = state.xwm.as_ref() {
                                XwmHandler::unfullscreen_request(state, xwm.id(), x11.clone());
                            }
                        }
                    }
                } else {
                    match active_window.underlying_surface() {
                        WindowSurface::Wayland(xdg) => {
                            XdgShellHandler::fullscreen_request(state, xdg.clone(), None);
                        }
                        #[cfg(feature = "xwayland")]
                        WindowSurface::X11(x11) => {
                            if let Some(xwm) = state.xwm.as_ref() {
                                XwmHandler::fullscreen_request(state, xwm.id(), x11.clone());
                            }
                        }
                    }
                }
            }
            Action::MoveWindowMouse => {
                state.init_pointer_move_grab(272, serial);
                state.refresh_layout();
            }
            Action::ResizeWindowMouse => {
                state.init_pointer_resize_grab(273, serial);
            }
            Action::PrevWorkspace => {
                state
                    .workspaces
                    .set_active_workspace(state.workspaces.prev_workspace);
            }
            Action::NextWorkspace => {
                state
                    .workspaces
                    .set_active_workspace(state.workspaces.active_workspace + 1);
            }
            Action::ResizeActive { direction, step } => {
                let ws = state.workspaces.get_current_mut();
                let active = match ws.get_active_window() {
                    Some(w) => w.clone(),
                    None => return,
                };
                let mode = match active.user_data().get::<RefCell<WindowMode>>() {
                    Some(m) => m.borrow().clone(),
                    None => return,
                };
                match mode {
                    WindowMode::Tiled => {}
                    WindowMode::Floating => {
                        let mut geo = ws.space.element_geometry(&active).unwrap();
                        match direction {
                            Direction::Top => {
                                geo.size.h += step;
                                geo.loc.y -= step;
                            }
                            Direction::Down => {
                                geo.size.h += step;
                            }
                            Direction::Left => {
                                geo.size.w += step;
                                geo.loc.x -= step;
                            }
                            Direction::Right => {
                                geo.size.w += step;
                            }
                        };
                        match active.underlying_surface() {
                            WindowSurface::Wayland(xdg) => {
                                xdg.with_pending_state(|state| state.size = Some(geo.size));
                                xdg.send_configure();
                            }
                            #[cfg(feature = "xwayland")]
                            WindowSurface::X11(x11) => {}
                        };
                        match direction {
                            Direction::Top | Direction::Left => {
                                ws.space.map_element(active, geo.loc, false);
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        };
    }
}
