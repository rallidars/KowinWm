use std::cell::RefCell;

use smithay::{
    delegate_data_control, delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, layer_map_for_output, PopupKeyboardGrab, PopupKind,
        PopupPointerGrab, PopupUngrabStrategy, Window, WindowSurfaceType,
    },
    input::{pointer::Focus, Seat},
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        seat::WaylandFocus,
        selection::wlr_data_control::{DataControlHandler, DataControlState},
        shell::xdg::{
            decoration::XdgDecorationHandler, Configure, PopupSurface, PositionerState,
            ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
    },
};

use crate::{
    state::State,
    utils::{
        grab::{MovePointerGrab, ResizePointerGrub},
        workspaces::{WindowMode, WindowUserData},
    },
};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        let ws = &mut self.workspaces.get_current_mut();
        let output = wl_output
            .clone()
            .and_then(|o| Output::from_resource(&o))
            .unwrap_or(ws.space.outputs().next().unwrap().clone());

        let window = ws
            .space
            .elements()
            .find(|w| w.toplevel().map(|s| s == &surface).unwrap_or(false))
            .cloned()
            .unwrap();
        window
            .user_data()
            .get::<RefCell<WindowUserData>>()
            .unwrap()
            .borrow_mut()
            .mode = WindowMode::Fullscreen(ws.space.element_geometry(&window).unwrap());
        ws.space.map_element(window.clone(), (0, 0), false);
        let geo = ws.space.output_geometry(&output).unwrap();
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geo.size);
            state.fullscreen_output = wl_output;
        });
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let ws = &mut self.workspaces.get_current_mut();
        let window = ws
            .space
            .elements()
            .find(|w| w.toplevel().map(|s| s == &surface).unwrap_or(false))
            .cloned()
            .unwrap();

        let window_mode = window
            .user_data()
            .get::<RefCell<WindowUserData>>()
            .unwrap()
            .borrow()
            .mode
            .clone();
        if let WindowMode::Fullscreen(prev_geo) = window_mode {
            surface.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Fullscreen);
                state.size = Some(prev_geo.size);
                state.fullscreen_output.take()
            });

            ws.space.map_element(window.clone(), prev_geo.loc, false);
            window
                .user_data()
                .get::<RefCell<WindowUserData>>()
                .unwrap()
                .borrow_mut()
                .mode = WindowMode::Tiled;

            surface.send_configure();

            self.refresh_layout();
        }
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.set_keyboard_focus(window.wl_surface().map(|s| s.as_ref().clone()));
        window.user_data().insert_if_missing(|| {
            RefCell::new(WindowUserData {
                mode: WindowMode::Tiled,
            })
        });
        self.workspaces.insert_window(window.clone());
        self.refresh_layout();
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        tracing::info!("new_popup");
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(surface.clone())) else {
            return;
        };

        let Some(window) = self
            .workspaces
            .get_current()
            .space
            .elements()
            .find(|w| w.wl_surface().unwrap().as_ref() == &root)
            .clone()
        else {
            return;
        };

        let mut outputs_for_window = self
            .workspaces
            .get_current()
            .space
            .outputs_for_element(&window);

        if outputs_for_window.is_empty() {
            return;
        }

        let window_geo = window.geometry();

        tracing::info!("geometry_new_popup: {:?}", window_geo);

        let geometry = positioner.get_unconstrained_geometry(window_geo);

        surface.with_pending_state(|state| {
            state.geometry = geometry;
        });
        if let Err(err) = self.popup_manager.track_popup(PopupKind::from(surface)) {
            tracing::warn!("Failed to track popup: {}", err);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let window = self
            .workspaces
            .get_current()
            .space
            .elements()
            .find(|w| {
                w.toplevel()
                    .map(|toplevel| toplevel == &surface)
                    .unwrap_or(false)
            })
            .unwrap()
            .clone();
        self.workspaces.remove_window(&window);
        self.workspaces.get_current_mut().active_window = None;
        self.refresh_layout();
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        if let Some(root) = find_popup_root_surface(&kind).ok().and_then(|root| {
            let ws = self.workspaces.get_current();
            ws.space
                .elements()
                .find(|w| w.wl_surface().map(|s| *s == root).unwrap_or(false))
                .cloned()
                .map(|w| w.wl_surface().unwrap().as_ref().clone())
                .or_else(|| {
                    ws.space.outputs().find_map(|o| {
                        let map = layer_map_for_output(o);
                        map.layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                            .cloned()
                            .map(|l| l.wl_surface().clone())
                    })
                })
        }) {
            let ret = self.popup_manager.grab_popup(root, kind, &seat, serial);

            if let Ok(mut grab) = ret {
                if let Some(keyboard) = seat.get_keyboard() {
                    if keyboard.is_grabbed()
                        && !(keyboard.has_grab(serial)
                            || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial)
                            || pointer
                                .has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
                }
            }
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        tracing::info!("new_popup_repo");
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner
        });
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(surface.clone())) else {
            return;
        };

        let Some(window) = self
            .workspaces
            .get_current()
            .space
            .elements()
            .find(|w| w.wl_surface().unwrap().as_ref() == &root)
            .clone()
        else {
            return;
        };

        let geometry = window.geometry();
        tracing::info!("geometry_reposition_request: {:?}", geometry);

        surface.with_pending_state(|state| {
            state.geometry = positioner.get_unconstrained_geometry(geometry);
        });

        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();

        let pointer = seat.get_pointer().unwrap();

        if !pointer.has_grab(serial) {
            return;
        }
        let ws = self.workspaces.get_current_mut();

        let window = match ws.space.elements().find(|element| {
            element
                .wl_surface()
                .map(|s| &*s == surface.wl_surface())
                .unwrap_or(false)
        }) {
            Some(w) => w.clone(),
            None => return,
        };

        let start_data = pointer.grab_start_data().unwrap();
        let start_loc = ws.space.element_location(&window).unwrap();

        let grab = MovePointerGrab {
            start_data,
            window,
            start_loc,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let pointer = seat.get_pointer().unwrap();

        if !pointer.has_grab(serial) {
            return;
        }
        let ws = self.workspaces.get_current_mut();

        let window = match ws.space.elements().find(|element| {
            element
                .wl_surface()
                .map(|s| &*s == surface.wl_surface())
                .unwrap_or(false)
        }) {
            Some(w) => w.clone(),
            None => return,
        };

        let start_data = pointer.grab_start_data().unwrap();

        let window_geo = match ws.space.element_geometry(&window) {
            Some(l) => l,
            None => return,
        };

        let grab = ResizePointerGrub {
            start_data,
            window,
            edges,
            start_geo: window_geo,
            last_window_size: window_geo.size,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
    fn ack_configure(
        &mut self,
        _surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        _configure: Configure,
    ) {
        tracing::info!("ack_configure request");
    }
}

delegate_xdg_shell!(State);

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        if let Some((serial, seat)) = data.serial {
            let keyboard = self.seat.get_keyboard().unwrap();
            Seat::from_resource(&seat) == Some(self.seat.clone())
                && keyboard
                    .last_enter()
                    .map(|last_enter| serial.is_no_older_than(&last_enter))
                    .unwrap_or(false)
        } else {
            false
        }
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed().as_secs() < 10 {
            // Just grant the wish
            let ws = self.workspaces.get_current_mut();
            let w = ws
                .space
                .elements()
                .find(|window| window.wl_surface().map(|s| *s == surface).unwrap_or(false))
                .cloned();
            if let Some(window) = w {
                ws.space.raise_element(&window, true);
            }
        }
    }
}
delegate_xdg_activation!(State);

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide)
        });
        toplevel.send_configure();
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = None;
        });
        toplevel.send_configure();
    }
    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        toplevel.send_configure();
    }
}
delegate_xdg_decoration!(State);

impl XdgForeignHandler for State {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.xdg_foreign_state
    }
}
smithay::delegate_xdg_foreign!(State);

impl DataControlHandler for State {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

delegate_data_control!(State);
