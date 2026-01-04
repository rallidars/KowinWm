use std::{ffi::OsString, sync::Arc, time::Instant};

use smithay::{
    backend::session::Session,
    desktop::{layer_map_for_output, PopupManager, Space, Window, WindowSurfaceType},
    input::{keyboard::XkbConfig, pointer::PointerHandle, Seat, SeatState},
    reexports::{
        calloop::{
            generic::Generic, EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction,
        },
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            Display, DisplayHandle,
        },
    },
    utils::{Logical, Point, Rectangle, Serial, SerialCounter, Size},
    wayland::{
        compositor::{self, CompositorClientState, CompositorState},
        input_method::InputMethodManagerState,
        seat::WaylandFocus,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::{
            wlr_layer::{self, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        socket::ListeningSocketSource,
        xdg_activation::XdgActivationState,
    },
};

use crate::{backend::udev::UdevData, utils::config::Config, utils::workspaces::Workspaces};
use crate::{utils::workspaces::is_fullscreen, SERIAL_COUNTER};

pub struct CalloopData {
    pub state: State,
    pub display_handle: DisplayHandle,
}

pub struct State {
    pub config: Config,
    pub backend_data: UdevData,
    pub loop_handle: LoopHandle<'static, CalloopData>,
    pub workspaces: Workspaces,
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
    pub compositor_state: CompositorState,
    pub pointer_location: Point<f64, Logical>,
    pub socket_name: OsString,

    pub xdg_shell_state: XdgShellState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_decoration_state: XdgDecorationState,

    pub pointer: PointerHandle<Self>,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub seat: Seat<Self>,
    pub loop_signal: LoopSignal,
    pub primary_selection_state: PrimarySelectionState,
    pub popup_manager: PopupManager,
    pub layer_shell_state: WlrLayerShellState,
    pub space: Space<Window>,
    pub current_layout: String,
}

impl State {
    pub fn new(
        loop_handle: LoopHandle<'static, CalloopData>,
        loop_signal: LoopSignal,
        display: Display<Self>,
        backend_data: UdevData,
    ) -> Self {
        let start_time = Instant::now();
        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let mut seat_state: SeatState<Self> = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let seat_name = backend_data.session.seat();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone());
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        //InputMethodManagerState::new::<Self, _>(&dh, |_client| true);
        let config = Config::get_config().unwrap_or_default();
        let current_layout = config
            .keyboard
            .layouts
            .get(0)
            .unwrap_or(&"us".to_string())
            .to_string();

        let xkb_config = XkbConfig {
            layout: &current_layout,
            ..Default::default()
        };
        seat.add_keyboard(xkb_config, 200, 25).unwrap();
        let pointer = seat.add_pointer();
        let listening_socket = ListeningSocketSource::new_auto().unwrap();
        let config = Config::get_config().unwrap_or_default();

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
            workspaces: Workspaces::new(config.workspaces),
            display_handle: dh,
            loop_signal,
            start_time,
            compositor_state,
            xdg_shell_state,
            xdg_activation_state,
            shm_state,
            seat_state,
            data_device_state,
            seat,
            socket_name,
            popup_manager: PopupManager::default(),
            xdg_decoration_state,
            primary_selection_state,
            layer_shell_state,
            space: Space::default(),
            config,
            current_layout,
        }
    }

    pub fn surface_under(
        &self,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<f64, Logical>,
    )> {
        let ws = self.workspaces.get_current();
        let pos = self.pointer_location;
        let output = self.space.outputs().find(|o| {
            let geometry = self.space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = self.space.output_geometry(output).unwrap();

        let mut under = None;
        let layers = layer_map_for_output(&output);

        if let Some(fullscreen) = is_fullscreen(self.space.elements()) {
            let geo = self.space.element_geometry(fullscreen).unwrap();
            under = fullscreen
                .wl_surface()
                .map(|s| (s.as_ref().clone(), geo.loc - fullscreen.geometry().loc));
        } else if let Some(layer) = layers
            .layer_under(wlr_layer::Layer::Overlay, pos)
            .or_else(|| layers.layer_under(wlr_layer::Layer::Top, pos))
            .or_else(|| {
                layers.layers().find(|l| {
                    l.layer() == wlr_layer::Layer::Overlay || l.layer() == wlr_layer::Layer::Top
                })
            })
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;

            // Relative position within the layer surface
            let relative_pos = pos - layer_loc.to_f64();

            // Find the actual wl_surface (and its local pos) under the relative position
            if let Some((surface, surface_loc)) =
                layer.surface_under(relative_pos, WindowSurfaceType::ALL)
            {
                under = Some((surface, surface_loc + layer_loc + output_geo.loc));
            }
        } else if let Some(data) = self.window_under() {
            under = Some(data);
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
                under = Some((surface, surface_loc + layer_loc + output_geo.loc));
            }
        }

        under.map(|(s, loc)| (s, loc.to_f64()))
    }
    fn window_under(
        &self,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<i32, Logical>,
    )> {
        let offset = self.config.border.gap + self.config.border.thickness;
        for element in self.space.elements() {
            let geo = self.space.element_geometry(element)?;
            let mut offset_geo = geo;
            offset_geo.size += (offset * 2, offset * 2).into();
            offset_geo.loc -= (offset, offset).into();
            if offset_geo.contains(self.pointer_location.to_i32_round()) {
                return element
                    .wl_surface()
                    .map(|s| (s.as_ref().clone(), geo.loc - element.geometry().loc));
            }
        }
        None
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
                .layout
                .iter()
                .find(|w| w.wl_surface().map(|s| *s == under).unwrap_or(false));
            self.workspaces.set_active_window(active.cloned());
            self.set_keyboard_focus(Some(under));
        }
    }

    pub fn refresh_layout(&mut self) {
        self.space.refresh();
        let offset = self.config.border.gap + self.config.border.thickness;
        let ws = &mut self.workspaces.get_current_mut();
        let active = ws.active_window.clone();

        let output_geometry = self.space.outputs().next().and_then(|o| {
            let geo = self.space.output_geometry(&o)?;
            let map = layer_map_for_output(&o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::new(geo.loc + zone.loc, zone.size))
        });
        let geo = match output_geometry {
            Some(g) => g,
            None => return,
        };

        let output_width = geo.size.w;
        let output_height = geo.size.h;

        let count = ws.layout.len() as i32;

        if count == 0 {
            return;
        }

        if let Some(_fs) = is_fullscreen(ws.layout.iter()) {
            return;
        }

        let half_width = output_width / 2;
        let vertical_height = if count > 1 {
            output_height / (count - 1)
        } else {
            output_height
        };
        let mut focus_window: Option<Window> = None;

        for (i, window) in ws.layout.iter().enumerate() {
            let (mut x, mut y) = (0, 0);
            let (mut width, mut height) = (output_width, output_height);

            if count > 1 {
                width = half_width;
            }

            if i > 0 {
                height = vertical_height;
                x = half_width;
                y = vertical_height * (i as i32 - 1);
            }
            let loc: Point<i32, Logical> = (x + offset, y + offset).into();
            let size: Size<i32, Logical> = (width - offset * 2, height - offset * 2).into();
            let geo = Rectangle::new(loc, size);
            if geo.contains(self.pointer_location.to_i32_round()) {
                focus_window = Some(window.clone());
            }

            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.bounds = output_geometry.map(|s| s.size);
                    state.states.set(xdg_toplevel::State::Maximized);
                    state.size = Some(geo.size);
                });
                toplevel.send_configure();
            }

            self.space.map_element(window.clone(), geo.loc, false);
        }

        if let None = active {
            ws.active_window = focus_window.clone();
            self.set_keyboard_focus(
                focus_window.and_then(|w| w.wl_surface().map(|s| s.as_ref().clone())),
            );
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
