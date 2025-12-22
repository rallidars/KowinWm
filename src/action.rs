use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::compositor::with_states;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::ToplevelSurface;
use smithay::{desktop::Window, wayland::shell::xdg::XdgShellHandler};

use crate::state::{Backend, State};

#[derive(PartialEq)]
pub enum Action {
    Spawm(String),
    Close,
    SetActiveWorkspace(usize),
    MoveWindowToWorkspace(usize),
    Quit,
    FullScreen,
    ChangeFocus(Direction),
    MoveWindow(Direction),
}

#[derive(PartialEq)]
pub enum Direction {
    Left,
    Right,
    Top,
    Down,
}

impl Action {
    pub fn execute<BackendData: Backend + 'static>(&self, state: &mut State<BackendData>) {
        match self {
            Action::Quit => {
                state.loop_signal.stop();
            }
            Action::Spawm(program) => {
                std::process::Command::new(program).spawn().unwrap();
            }
            Action::Close => {
                let under = state.surface_under().map(|w| w.0);
                let toplevel = state
                    .workspaces
                    .get_current()
                    .space
                    .elements()
                    .find(|w| w.wl_surface().as_deref() == under.as_ref())
                    .and_then(|w| w.toplevel());
                if let Some(toplevel) = toplevel {
                    toplevel.send_close();
                }
            }
            Action::SetActiveWorkspace(ws_id) => {
                state.workspaces.set_active_workspace(*ws_id);
                state.set_keyboard_focus_auto();
            }
            Action::MoveWindowToWorkspace(ws_index) => {
                state.workspaces.move_window_to_ws(*ws_index);
            }
            Action::MoveWindow(direction) => {
                state.workspaces.move_window(direction);
                state.set_keyboard_focus_auto();
            }
            Action::ChangeFocus(direction) => {
                state
                    .workspaces
                    .change_focus(direction, &mut state.pointer_location);
                state.set_keyboard_focus_auto();
            }
            Action::FullScreen => {
                //let current_ws = state.workspaces.get_current();
                //let current_win = match current_ws.active_window {
                //    Some(w) => w,
                //    None => return,
                //};
                //let surface = match current_ws
                //    .space
                //    .elements()
                //    .nth(current_win)
                //    .and_then(|w| w.toplevel())
                //{
                //    Some(s) => s,
                //    None => return,
                //};
                //if surface
                //    .current_state()
                //    .states
                //    .contains(xdg_toplevel::State::Fullscreen)
                //{
                //    state.unfullscreen_request(surface.clone());
                //} else {
                //    state.fullscreen_request(surface.clone(), None);
                //}
                //state.workspaces.layout();
            }
        };
    }
}
