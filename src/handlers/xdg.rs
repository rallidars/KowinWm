use smithay::{
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{find_popup_root_surface, get_popup_toplevel_coords, PopupKind, Window},
    input::Seat,
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel, wayland_server::protocol::wl_seat,
    },
    utils::Serial,
    wayland::{
        seat::WaylandFocus,
        shell::xdg::{
            decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface,
            XdgShellHandler, XdgShellState,
        },
    },
};

use crate::state::{Backend, State};

impl<BackendData: Backend + 'static> XdgShellHandler for State<BackendData> {
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
            .unwrap_or(
                self.workspaces
                    .get_current()
                    .space
                    .outputs()
                    .next()
                    .unwrap()
                    .clone(),
            );

        let geo = self.workspaces.get_current().space.output_geometry(&output);
        let size = geo.and_then(|g| Some(g.size));
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = size;
            state.fullscreen_output = wl_output;
        });
        surface.send_configure();
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
            state.size = None;
            state.fullscreen_output.take()
        });
        surface.send_configure();
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.set_keyboard_focus(Some(surface.wl_surface().clone()));
        let window = Window::new_wayland_window(surface);
        self.workspaces.insert_window(window);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
        });
        if let Err(err) = self.popup_manager.track_popup(PopupKind::from(surface)) {
            tracing::warn!("Failed to track popup: {}", err);
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        self.workspaces.remove_active_window(window);
        self.set_keyboard_focus_auto();
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let seat: Seat<State<BackendData>> = Seat::from_resource(&seat).unwrap();
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

        surface.send_repositioned(token);
    }
}

delegate_xdg_shell!(@<BackendData: Backend + 'static> State<BackendData>);

impl<BackendData: Backend + 'static> XdgDecorationHandler for State<BackendData> {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode::ServerSide)

        });
        toplevel.send_configure();
    }
    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
    fn request_mode(
        &mut self,
        _toplevel: ToplevelSurface,
        _mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
    }
}
delegate_xdg_decoration!(@<BackendData: Backend + 'static> State<BackendData>);
