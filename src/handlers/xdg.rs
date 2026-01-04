use smithay::{
    delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{find_popup_root_surface, PopupKind, Window},
    input::Seat,
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::{wl_seat, wl_surface::WlSurface},
    },
    utils::Serial,
    wayland::{
        compositor::with_states,
        seat::WaylandFocus,
        shell::xdg::{
            decoration::XdgDecorationHandler, Configure, PopupSurface, PositionerState,
            ToplevelConfigure, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::{state::State, utils::workspaces::window_center};

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }
    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        wl_output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        let output = wl_output
            .clone()
            .and_then(|o| Output::from_resource(&o))
            .unwrap_or(self.space.outputs().next().unwrap().clone());
        let ws = &mut self.workspaces.get_current_mut();

        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().map(|s| s == &surface).unwrap_or(false))
            .cloned()
            .unwrap();
        ws.full_geo = self.space.element_geometry(&window);

        self.space.map_element(window.clone(), (0, 0), false);
        let geo = self.space.output_geometry(&output).unwrap();
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(geo.size);
            state.fullscreen_output = wl_output;
        });
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let ws = &mut self.workspaces.get_current_mut();
        let window = self
            .space
            .elements()
            .find(|w| w.toplevel().map(|s| s == &surface).unwrap_or(false))
            .cloned()
            .unwrap();
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = ws.full_geo.map(|w| w.size);
            state.fullscreen_output.take()
        });
        self.space
            .map_element(window, ws.full_geo.unwrap().loc, false);
        self.workspaces.get_current_mut().full_geo = None;
        surface.send_configure();
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.set_keyboard_focus(window.wl_surface().map(|s| s.as_ref().clone()));
        self.workspaces.insert_window(window.clone());
        self.refresh_layout();
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        tracing::info!("new_popup");
        let Ok(root) = find_popup_root_surface(&PopupKind::from(surface.clone())) else {
            return;
        };

        let Some(window) = self
            .workspaces
            .get_current()
            .layout
            .iter()
            .find(|w| w.wl_surface().unwrap().as_ref() == &root)
            .clone()
        else {
            return;
        };

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
            .layout
            .iter()
            .find(|w| w.toplevel().unwrap() == &surface)
            .unwrap()
            .clone();
        self.workspaces.remove_window(&window, &mut self.space);
        self.workspaces.get_current_mut().active_window = None;
        self.refresh_layout();
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        tracing::info!("geometry_reposition_requesty");
        let seat: Seat<State> = Seat::from_resource(&seat).unwrap();
        let kind = PopupKind::Xdg(surface);
        let root = find_popup_root_surface(&kind).unwrap();
        self.popup_manager
            .grab_popup(root, kind, &seat, serial)
            .unwrap();
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
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
            .layout
            .iter()
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
            let w = self
                .space
                .elements()
                .find(|window| window.wl_surface().map(|s| *s == surface).unwrap_or(false))
                .cloned();
            if let Some(window) = w {
                self.space.raise_element(&window, true);
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
