mod input;
mod xdg;
#[cfg(feature = "xwayland")]
mod xwayland;

use std::{os::fd::OwnedFd, sync::Mutex};

use crate::state::{ClientState, State};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output,
    delegate_primary_selection, delegate_seat, delegate_shm,
    desktop::{
        layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{Seat, SeatHandler, SeatState},
    output::Output,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client, Resource,
    },
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
            xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceRoleAttributes},
        },
        shm::{ShmHandler, ShmState},
    },
};

delegate_compositor!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_data_device!(State);
delegate_output!(State);

impl OutputHandler for State {}

impl BufferHandler for State {
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
                .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
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

            PopupKind::InputMethod(ref _input_popup) => {
                return;
            }
        };

        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgPopupSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            // NOTE: This should never fail as the initial configure is always
            // allowed.
            popup.send_configure().expect("initial configure failed");
        }
    };
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        let ws = self.workspaces.get_current();
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = ws
                .space
                .elements()
                .find(|w| w.toplevel().unwrap().wl_surface() == &root)
            {
                window.on_commit();
            }
        };
        self.popup_manager.commit(surface);
        handle_commit(&ws.space, surface, &self.popup_manager);
        //self.set_keyboard_focus_auto();
    }
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl SeatHandler for State {
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
        let ws = self.workspaces.get_current();
        if let Some(w) = ws
            .space
            .elements()
            .find(|w| w.wl_surface().as_deref() == focused)
        {
            for window in ws.space.elements() {
                if window.eq(w) {
                    window.set_activated(true);
                } else {
                    window.set_activated(false);
                }
                window.toplevel().unwrap().send_configure();
            }
        }
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl WlrLayerShellHandler for State {
    fn new_layer_surface(
        &mut self,
        surface: smithay::wayland::shell::wlr_layer::LayerSurface,
        output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
        _layer: smithay::wayland::shell::wlr_layer::Layer,
        namespace: String,
    ) {
        let ws = self.workspaces.get_current();
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .unwrap_or_else(|| ws.space.outputs().next().unwrap().clone());
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
        let ws = self.workspaces.get_current();
        if let Some((mut map, layer)) = ws.space.outputs().find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map
                .layers()
                .find(|&layer| layer.layer_surface() == &surface)
                .cloned();
            layer.map(|layer| (map, layer))
        }) {
            map.unmap_layer(&layer);
        }
        self.set_keyboard_focus_auto();
    }
    fn new_popup(
        &mut self,
        parent: smithay::wayland::shell::wlr_layer::LayerSurface,
        popup: smithay::wayland::shell::xdg::PopupSurface,
    ) {
        tracing::info!("layer_new_popup")
    }
}
delegate_layer_shell!(State);

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(State);

//impl InputMethodHandler for State {
//    fn new_popup(&mut self, surface: PopupSurface) {
//        tracing::info!("new_popup");
//        let Ok(root) = find_popup_root_surface(&PopupKind::from(surface.clone())) else {
//            return;
//        };
//
//        let Some(window) = self
//            .workspaces
//            .get_current()
//            .layout
//            .iter()
//            .find(|w| w.wl_surface().unwrap().as_ref() == &root)
//            .clone()
//        else {
//            return;
//        };
//
//        let window_geo = window.geometry();
//
//        tracing::info!("geometry_new_popup: {:?}", window_geo);
//
//        let geometry = positioner.get_unconstrained_geometry(window_geo);
//
//        surface.with_pending_state(|state| {
//            state.geometry = geometry;
//        });
//        if let Err(err) = self.popup_manager.track_popup(PopupKind::from(surface)) {
//            tracing::warn!("Failed to track popup: {}", err);
//        }
//    }
//
//    fn popup_repositioned(&mut self, _: PopupSurface) {}
//
//    fn dismiss_popup(&mut self, surface: PopupSurface) {
//        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
//            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
//        }
//    }
//
//    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, smithay::utils::Logical> {
//        self.space
//            .elements()
//            .find_map(|window| {
//                (window.wl_surface().as_deref() == Some(parent)).then(|| window.geometry())
//            })
//            .unwrap_or_default()
//    }
//}
//
//delegate_input_method_manager!(State);
