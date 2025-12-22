mod xdg;

use std::os::fd::OwnedFd;

use crate::state::{Backend, ClientState, State};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output,
    delegate_primary_selection, delegate_seat, delegate_shm, delegate_xdg_decoration,
    delegate_xdg_shell,
    desktop::{
        layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{
                wl_buffer, wl_seat, wl_shell_surface::FullscreenMethod, wl_surface::WlSurface,
            },
            Client, Resource,
        },
    },
    utils::{Serial, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        output::OutputHandler,
        seat::WaylandFocus,
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            SelectionHandler,
        },
        shell::{
            wlr_layer::{LayerSurfaceData, WlrLayerShellHandler},
            xdg::{
                decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface,
                XdgPopupSurfaceData, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
            },
        },
        shm::{ShmHandler, ShmState},
    },
};

delegate_compositor!(@<BackendData: Backend + 'static> State<BackendData>);
delegate_shm!(@<BackendData: Backend + 'static> State<BackendData>);
delegate_seat!(@<BackendData: Backend + 'static> State<BackendData>);
delegate_data_device!(@<BackendData: Backend + 'static> State<BackendData>);
delegate_output!(@<BackendData: Backend + 'static> State<BackendData>);

impl<BackendData: Backend + 'static> OutputHandler for State<BackendData> {}

impl<BackendData: Backend + 'static> BufferHandler for State<BackendData> {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

pub fn handle_commit(space: &Space<Window>, surface: &WlSurface, popup_manager: &PopupManager) {
    // Handle toplevel commits.

    if let Some(window) = space
        .elements()
        .find(|w| w.toplevel().unwrap().wl_surface() == surface)
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    if let Some(output) = space.outputs().find(|o| {
        let map = layer_map_for_output(o);
        map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some()
    }) {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        let mut map = layer_map_for_output(output);

        // arrange the layers before sending the initial configure
        // to respect any size the client may have sent
        map.arrange();
        // send the initial configure if relevant
        if !initial_configure_sent {
            let layer = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .unwrap();

            layer.layer_surface().send_configure();
        }
    };

    if let Some(popup) = popup_manager.find_popup(surface) {
        let popup = match popup {
            PopupKind::Xdg(ref popup) => popup,
            // Doesn't require configure
            PopupKind::InputMethod(ref _input_popup) => {
                return;
            }
        };

        if !popup.is_initial_configure_sent() {
            popup.send_configure().expect("initial configure failed");
        }

        return;
    };
}

impl<BackendData: Backend + 'static> SelectionHandler for State<BackendData> {
    type SelectionUserData = ();
}

impl<BackendData: Backend + 'static> DataDeviceHandler for State<BackendData> {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl<BackendData: Backend + 'static> ClientDndGrabHandler for State<BackendData> {}
impl<BackendData: Backend + 'static> ServerDndGrabHandler for State<BackendData> {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl<BackendData: Backend + 'static> CompositorHandler for State<BackendData> {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .workspaces
                .get_current()
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == &root)
            {
                window.on_commit();
            }
        };
        self.popup_manager.commit(surface);
        handle_commit(
            &self.workspaces.get_current().space,
            surface,
            &self.popup_manager,
        );
        self.set_keyboard_focus_auto();
    }
}

impl<BackendData: Backend + 'static> ShmHandler for State<BackendData> {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl<BackendData: Backend + 'static> SeatHandler for State<BackendData> {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn led_state_changed(
        &mut self,
        _seat: &Seat<Self>,
        _led_state: smithay::input::keyboard::LedState,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let dh = &self.display_handle;
        let focus = focused
            .and_then(WaylandFocus::wl_surface)
            .and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, focus.clone());
        set_primary_focus(dh, seat, focus);
        //if let Some(w) = self
        //    .workspaces
        //    .get_current()
        //    .space
        //    .elements()
        //    .find(|w| w.wl_surface().as_deref() == focused)
        //{
        //    for window in self.workspaces.get_current().space.elements() {
        //        if window.eq(w) {
        //            window.set_activated(true);
        //        } else {
        //            window.set_activated(false);
        //        }
        //        window.toplevel().unwrap().send_configure();
        //    }
        //}
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl<BackendData: Backend + 'static> WlrLayerShellHandler for State<BackendData> {
    fn new_layer_surface(
        &mut self,
        surface: smithay::wayland::shell::wlr_layer::LayerSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        layer: smithay::wayland::shell::wlr_layer::Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| {
                self.workspaces
                    .get_current()
                    .space
                    .outputs()
                    .next()
                    .unwrap()
                    .clone()
            });
        let mut map = layer_map_for_output(&output);
        let layer_surface = LayerSurface::new(surface, namespace);
        map.map_layer(&layer_surface).unwrap();
        drop(map);
        self.set_keyboard_focus(Some(layer_surface.wl_surface().clone()));
    }
    fn shell_state(&mut self) -> &mut smithay::wayland::shell::wlr_layer::WlrLayerShellState {
        &mut self.layer_shell_state
    }
    fn layer_destroyed(&mut self, surface: smithay::wayland::shell::wlr_layer::LayerSurface) {
        if let Some((mut map, layer)) =
            self.workspaces.get_current().space.outputs().find_map(|o| {
                let map = layer_map_for_output(o);
                let layer = map
                    .layers()
                    .find(|&layer| layer.layer_surface() == &surface)
                    .cloned();
                layer.map(|layer| (map, layer))
            })
        {
            map.unmap_layer(&layer);
        }
        self.set_keyboard_focus_auto();
    }
}
delegate_layer_shell!(@<BackendData: Backend + 'static> State<BackendData>);

impl<BackendData: Backend> PrimarySelectionHandler for State<BackendData> {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(@<BackendData: Backend + 'static> State<BackendData>);
