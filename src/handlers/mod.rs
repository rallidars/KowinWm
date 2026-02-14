mod input;
mod xdg;
#[cfg(feature = "xwayland")]
mod xwayland;

use std::{os::fd::OwnedFd, sync::Mutex};

use crate::state::{ClientState, State};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_fractional_scale,
    delegate_input_method_manager, delegate_layer_shell, delegate_output, delegate_presentation,
    delegate_primary_selection, delegate_seat, delegate_shm, delegate_single_pixel_buffer,
    delegate_viewporter,
    desktop::{
        layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        calloop::Interest,
        wayland_server::{
            protocol::{wl_buffer, wl_surface::WlSurface},
            Client, Resource,
        },
    },
    utils::Rectangle,
    wayland::{
        buffer::BufferHandler,
        compositor::{
            add_blocker, add_pre_commit_hook, get_parent, is_sync_subsurface, with_states,
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes,
        },
        dmabuf::get_dmabuf,
        drm_syncobj::DrmSyncobjCachedState,
        fractional_scale::FractionalScaleHandler,
        input_method::InputMethodHandler,
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
            xdg::{XdgPopupSurfaceData, XdgToplevelSurfaceData, XdgToplevelSurfaceRoleAttributes},
        },
        shm::{ShmHandler, ShmState},
    },
};
#[cfg(feature = "xwayland")]
use smithay::{
    wayland::selection::{SelectionSource, SelectionTarget},
    xwayland::XWaylandClientData,
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
        .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
        .cloned()
    {
        #[cfg_attr(not(feature = "xwayland"), allow(irrefutable_let_patterns))]
        if let Some(toplevel) = window.toplevel() {
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
                toplevel.send_configure();
            }
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

        if !popup.is_initial_configure_sent() {
            // NOTE: This should never fail as the initial configure is always
            // allowed.
            popup.send_configure().expect("initial configure failed");
        }
    };
}

impl SelectionHandler for State {
    type SelectionUserData = ();

    #[cfg(feature = "xwayland")]
    fn new_selection(
        &mut self,
        ty: SelectionTarget,
        source: Option<SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.new_selection(ty, source.map(|source| source.mime_types())) {
                tracing::warn!(?err, ?ty, "Failed to set Xwayland selection");
            }
        }
    }

    #[cfg(feature = "xwayland")]
    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &(),
    ) {
        if let Some(xwm) = self.xwm.as_mut() {
            if let Err(err) = xwm.send_selection(ty, mime_type, fd, self.loop_handle.clone()) {
                tracing::warn!(?err, "Failed to send primary (X11 -> Wayland)");
            }
        }
    }
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        #[cfg(feature = "xwayland")]
        if let Some(state) = client.get_data::<XWaylandClientData>() {
            return &state.compositor_state;
        }
        if let Some(state) = client.get_data::<ClientState>() {
            return &state.compositor_state;
        }
        panic!("Unknown client data type")
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        add_pre_commit_hook::<Self, _>(surface, move |state, _dh, surface| {
            let mut acquire_point = None;
            let maybe_dmabuf = with_states(surface, |surface_data| {
                acquire_point.clone_from(
                    &surface_data
                        .cached_state
                        .get::<DrmSyncobjCachedState>()
                        .pending()
                        .acquire_point,
                );
                surface_data
                    .cached_state
                    .get::<SurfaceAttributes>()
                    .pending()
                    .buffer
                    .as_ref()
                    .and_then(|assignment| match assignment {
                        BufferAssignment::NewBuffer(buffer) => get_dmabuf(buffer).cloned().ok(),
                        _ => None,
                    })
            });
            if let Some(dmabuf) = maybe_dmabuf {
                if let Some(acquire_point) = acquire_point {
                    if let Ok((blocker, source)) = acquire_point.generate_blocker() {
                        let client = surface.client().unwrap();
                        let res = state.loop_handle.insert_source(source, move |_, _, data| {
                            let dh = data.display_handle.clone();
                            data.client_compositor_state(&client)
                                .blocker_cleared(data, &dh);
                            Ok(())
                        });
                        if res.is_ok() {
                            add_blocker(surface, blocker);
                            return;
                        }
                    }
                }
                if let Ok((blocker, source)) = dmabuf.generate_blocker(Interest::READ) {
                    if let Some(client) = surface.client() {
                        let res = state.loop_handle.insert_source(source, move |_, _, data| {
                            let dh = data.display_handle.clone();
                            data.client_compositor_state(&client)
                                .blocker_cleared(data, &dh);
                            Ok(())
                        });
                        if res.is_ok() {
                            add_blocker(surface, blocker);
                        }
                    }
                }
            }
        });
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.backend_data.early_import(surface);

        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self.window_for_surface(surface) {
                window.on_commit();

                if &root == surface {
                    let buffer_offset = with_states(surface, |states| {
                        states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .current()
                            .buffer_delta
                            .take()
                    });

                    if let Some(buffer_offset) = buffer_offset {
                        let ws = self.workspaces.get_current_mut();
                        let current_loc = ws.space.element_location(&window).unwrap();
                        ws.space
                            .map_element(window.clone(), current_loc + buffer_offset, false);
                    }
                }
            }
        };
        self.popup_manager.commit(surface);
        handle_commit(
            &self.workspaces.get_current().space,
            surface,
            &self.popup_manager,
        );
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
        //let ws = self.workspaces.get_current();
        //if let Some(w) = ws
        //    .space
        //    .elements()
        //    .find(|w| w.wl_surface().as_deref() == focused)
        //{
        //    for window in ws.space.elements() {
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
        let layer_surface = &LayerSurface::new(surface, namespace);
        map.map_layer(&layer_surface).unwrap();
        drop(map);
        self.set_keyboard_focus(Some(layer_surface.wl_surface().clone()));
        self.refresh_layout();
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
        self.refresh_layout();
    }
    fn new_popup(
        &mut self,
        parent: smithay::wayland::shell::wlr_layer::LayerSurface,
        popup: smithay::wayland::shell::xdg::PopupSurface,
    ) {
        tracing::info!("layer popup");

        let ws = self.workspaces.get_current();
        let output = ws.space.outputs().next().unwrap();
        let outptut_geo = ws.space.output_geometry(output).unwrap();
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(outptut_geo);
        });

        if let Err(err) = self
            .popup_manager
            .track_popup(PopupKind::from(popup.clone()))
        {
            tracing::warn!("Failed to track popup: {}", err);
        }
    }
}

delegate_layer_shell!(State);

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(State);

delegate_viewporter!(State);
delegate_single_pixel_buffer!(State);

impl InputMethodHandler for State {
    fn new_popup(&mut self, surface: smithay::wayland::input_method::PopupSurface) {
        if let Err(err) = self.popup_manager.track_popup(PopupKind::from(surface)) {
            tracing::warn!("Failed to track popup: {}", err);
        }
    }

    fn popup_repositioned(&mut self, _: smithay::wayland::input_method::PopupSurface) {}

    fn dismiss_popup(&mut self, surface: smithay::wayland::input_method::PopupSurface) {
        if let Some(parent) = surface.get_parent().map(|parent| parent.surface.clone()) {
            let _ = PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, smithay::utils::Logical> {
        self.workspaces
            .get_current()
            .space
            .elements()
            .find_map(|window| {
                (window.wl_surface().as_deref() == Some(parent)).then(|| window.geometry())
            })
            .unwrap_or_default()
    }
}

delegate_input_method_manager!(State);
