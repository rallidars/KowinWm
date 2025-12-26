use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::shell::xdg::XdgShellHandler;

use crate::state::{Backend, State};
use crate::workspaces::is_fullscreen;

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
                state
                    .workspaces
                    .move_window(direction, &mut state.pointer_location);
                state.set_keyboard_focus_auto();
            }
            Action::ChangeFocus(direction) => {
                state
                    .workspaces
                    .change_focus(direction, &mut state.pointer_location);
                state.set_keyboard_focus_auto();
            }
            Action::FullScreen => {
                let acitve_window = match &state.workspaces.get_current().active_window {
                    Some(active) => active,
                    None => return,
                };
                let elements = state.workspaces.get_current().space.elements();
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
