use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    desktop::{layer_map_for_output, PopupManager, WindowSurfaceType},
    input::{keyboard::XkbConfig, pointer::PointerHandle, Seat, SeatState},
    reexports::{
        calloop::{
            generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, Serial, SerialCounter},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        seat::WaylandFocus,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::{
            wlr_layer::{self, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
    },
};

use crate::workspaces::Workspaces;
use crate::SERIAL_COUNTER;

pub struct CalloopData<BackendData: Backend + 'static> {
    pub state: State<BackendData>,
    pub display_handle: DisplayHandle,
}

pub trait Backend {
    fn seat_name(&self) -> String;
}

pub struct State<BackendData: Backend + 'static> {
    pub backend_data: BackendData,
    pub loop_handle: LoopHandle<'static, CalloopData<BackendData>>,
    pub workspaces: Workspaces,
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
    pub compositor_state: CompositorState,
    pub pointer_location: Point<f64, Logical>,
    pub socket_name: OsString,
    pub xdg_shell_state: XdgShellState,
    pub pointer: PointerHandle<Self>,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,
    pub loop_signal: LoopSignal,
    pub primary_selection_state: PrimarySelectionState,
    pub popup_manager: PopupManager,
    pub xdg_decoration_state: XdgDecorationState,
    pub layer_shell_state: WlrLayerShellState,
}

impl<BackendData: Backend + 'static> State<BackendData> {
    pub fn new(
        loop_handle: LoopHandle<'static, CalloopData<BackendData>>,
        loop_signal: LoopSignal,
        display: Display<Self>,
        backend_data: BackendData,
    ) -> Self {
        let start_time = Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let mut seat_state: SeatState<Self> = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let seat_name = backend_data.seat_name();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone());
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);

        seat.add_keyboard(XkbConfig::default(), 200, 25).unwrap();
        let pointer = seat.add_pointer();
        let listening_socket = ListeningSocketSource::new_auto().unwrap();

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    unsafe {
                        display
                            .get_mut()
                            .dispatch_clients(&mut state.state)
                            .unwrap()
                    };
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init wayland server source");

        Self {
            pointer_location: (0.0, 0.0).into(),
            pointer,
            backend_data,
            loop_handle,
            workspaces: Workspaces::new(),
            display_handle: dh,
            loop_signal,
            start_time,
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            seat,
            socket_name,
            popup_manager: PopupManager::default(),
            xdg_decoration_state,
            primary_selection_state,
            layer_shell_state,
        }
    }

    pub fn surface_under(
        &self,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<f64, Logical>,
    )> {
        let pos = self.pointer_location;
        let space = &self.workspaces.get_current().space;
        let output = space.outputs().find(|o| {
            let geometry = space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = space.output_geometry(output).unwrap();

        let mut under = None;
        let layers = layer_map_for_output(&output);

        // Search from highest to lowest layer for proper occlusion
        if let Some(layer) = layers
            .layer_under(wlr_layer::Layer::Overlay, pos)
            .or_else(|| layers.layer_under(wlr_layer::Layer::Top, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;

            // Relative position within the layer surface
            let relative_pos = pos - layer_loc.to_f64();

            // Find the actual wl_surface (and its local pos) under the relative position
            if let Some((surface, surface_loc)) =
                layer.surface_under(relative_pos, WindowSurfaceType::ALL)
            {
                under = Some((
                    surface,
                    (surface_loc + layer_loc).to_f64() + output_geo.loc.to_f64(),
                ));
            }
        } else if let Some(focus) = space.element_under(pos).and_then(|(window, loc)| {
            window
                .surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, surf_loc)| (surface, surf_loc + loc))
        }) {
            under = Some((focus.0, focus.1.to_f64()));
        } else if let Some(layer) = layers
            .layer_under(wlr_layer::Layer::Bottom, pos)
            .or_else(|| layers.layer_under(wlr_layer::Layer::Background, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;

            // Relative position within the layer surface
            let relative_pos = pos - layer_loc.to_f64();

            // Find the actual wl_surface (and its local pos) under the relative position
            if let Some((surface, surface_loc)) =
                layer.surface_under(relative_pos, WindowSurfaceType::ALL)
            {
                under = Some((
                    surface,
                    (surface_loc + layer_loc).to_f64() + output_geo.loc.to_f64(),
                ));
            }
        }

        under
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
    }

    pub fn set_keyboard_focus_auto(&mut self) {
        if let Some(under) = self.surface_under().map(|s| s.0) {
            let active = self
                .workspaces
                .get_current()
                .space
                .elements()
                .find(|w| w.wl_surface().map(|s| *s == under).unwrap_or(false));
            self.workspaces.set_active_window(active.cloned());
            self.set_keyboard_focus(Some(under));
        }
    }
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
