use std::{
    cell::RefCell,
    ffi::OsString,
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

use smithay::{
    backend::session::Session,
    desktop::{
        layer_map_for_output, space::SpaceElement, PopupKind, PopupManager, Space, Window,
        WindowSurface, WindowSurfaceType,
    },
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
    utils::{Clock, Logical, Monotonic, Point, Rectangle, Serial, SerialCounter, Size},
    wayland::{
        compositor::{self, CompositorClientState, CompositorState},
        input_method::InputMethodManagerState,
        output::OutputManagerState,
        seat::WaylandFocus,
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState,
        },
        shell::{
            wlr_layer::{self, WlrLayerShellState},
            xdg::{decoration::XdgDecorationState, XdgShellState},
        },
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
    },
};

#[cfg(feature = "xwayland")]
use smithay::{
    wayland::{xwayland_keyboard_grab::XWaylandKeyboardGrabState, xwayland_shell},
    xwayland::X11Wm,
    xwayland::XWaylandEvent,
};

use crate::{
    udev::UdevData,
    utils::{
        config::Config,
        layout::LayoutBehavior,
        workspaces::{place_on_center, WindowMode, WindowUserData, Workspaces},
    },
};
use crate::{utils::workspaces::is_fullscreen, SERIAL_COUNTER};

pub struct CalloopData {
    pub state: State,
    pub display_handle: DisplayHandle,
}

pub struct State {
    pub clock: Clock<Monotonic>,

    //something idk
    pub viewporter_state: ViewporterState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub data_control_state: DataControlState,

    //basics
    pub running: Arc<AtomicBool>,

    pub config: Config,
    pub backend_data: UdevData,
    pub loop_handle: LoopHandle<'static, State>,
    pub workspaces: Workspaces,
    pub display_handle: DisplayHandle,
    pub start_time: Instant,
    pub compositor_state: CompositorState,
    pub pointer_location: Point<f64, Logical>,
    pub socket_name: OsString,

    pub output_manager_state: OutputManagerState,

    pub xdg_foreign_state: XdgForeignState,
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
    pub current_layout: String,

    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state: xwayland_shell::XWaylandShellState,

    #[cfg(feature = "xwayland")]
    pub xwm: Option<X11Wm>,
    #[cfg(feature = "xwayland")]
    pub xdisplay: Option<u32>,
}

impl State {
    pub fn new(
        loop_handle: LoopHandle<'static, State>,
        loop_signal: LoopSignal,
        display: Display<Self>,
        backend_data: UdevData,
    ) -> Self {
        let clock = Clock::new();

        let start_time = Instant::now();
        let dh = display.handle();

        //someething idk
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&dh);

        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);

        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let mut seat_state: SeatState<Self> = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let seat_name = backend_data.session.seat();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone());
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let data_control_state =
            DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), |_| true);

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

        InputMethodManagerState::new::<Self, _>(&dh, |_client| true);

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = xwayland_shell::XWaylandShellState::new::<Self>(&dh.clone());

        #[cfg(feature = "xwayland")]
        XWaylandKeyboardGrabState::new::<Self>(&dh.clone());

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
                    unsafe { display.get_mut().dispatch_clients(state).unwrap() };
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init wayland server source");

        Self {
            clock,

            viewporter_state,
            single_pixel_buffer_state,
            data_control_state,

            running: Arc::new(AtomicBool::new(true)),

            output_manager_state,
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
            xdg_foreign_state,

            shm_state,
            seat_state,
            data_device_state,
            seat,
            socket_name,
            popup_manager: PopupManager::default(),
            xdg_decoration_state,
            primary_selection_state,
            layer_shell_state,
            config,
            current_layout,

            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            #[cfg(feature = "xwayland")]
            xwm: None,
            #[cfg(feature = "xwayland")]
            xdisplay: None,
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
        let output = ws.space.outputs().find(|o| {
            let geometry = ws.space.output_geometry(o).unwrap();
            geometry.contains(pos.to_i32_round())
        })?;
        let output_geo = ws.space.output_geometry(output).unwrap();

        let mut under = None;
        let layers = layer_map_for_output(&output);

        if let Some(fullscreen) = is_fullscreen(ws.space.elements()) {
            let geo = ws.space.element_geometry(fullscreen).unwrap();
            under = fullscreen
                .wl_surface()
                .map(|s| (s.as_ref().clone(), geo.loc - fullscreen.geometry().loc));
        } else if let Some(layer) = layers
            .layer_under(wlr_layer::Layer::Overlay, pos)
            .or_else(|| layers.layer_under(wlr_layer::Layer::Top, pos))
        //.or_else(|| {
        //    layers.layers().find(|l| {
        //        l.layer() == wlr_layer::Layer::Overlay || l.layer() == wlr_layer::Layer::Top
        //    })
        //})
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(
                    pos - output_geo.loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, loc)| (surface, loc + layer_loc + output_geo.loc));
        } else if let Some(data) = self.window_under() {
            under = Some(data);
        } else if let Some(layer) = layers
            .layer_under(wlr_layer::Layer::Bottom, pos)
            .or_else(|| layers.layer_under(wlr_layer::Layer::Background, pos))
        {
            let layer_loc = layers.layer_geometry(layer).unwrap().loc;
            under = layer
                .surface_under(
                    pos - output_geo.loc.to_f64() - layer_loc.to_f64(),
                    WindowSurfaceType::ALL,
                )
                .map(|(surface, loc)| (surface, loc + layer_loc + output_geo.loc));
        }

        under.map(|(s, loc)| (s, loc.to_f64()))
    }

    fn window_under(
        &self,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<i32, Logical>,
    )> {
        let ws = self.workspaces.get_current();

        let mut tiled_hit = None;

        // Iterate top → bottom (important for correct stacking)
        for element in ws.space.elements().rev() {
            if let Some(hit) = self.window_contains_pointer(element) {
                match element
                    .user_data()
                    .get::<RefCell<WindowUserData>>()
                    .unwrap()
                    .borrow()
                    .mode
                {
                    // Floating windows always win immediately
                    WindowMode::Floating => return Some(hit),

                    // Remember first tiled hit (topmost tiled)
                    WindowMode::Tiled if tiled_hit.is_none() => {
                        tiled_hit = Some(hit);
                    }

                    _ => {}
                }
            }
        }

        tiled_hit
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
    }

    pub fn set_keyboard_focus_auto(&mut self) {
        if let Some(under) = self.surface_under().map(|s| s.0) {
            let ws = self.workspaces.get_current_mut();
            let active = ws
                .space
                .elements()
                .find(|w| w.wl_surface().map(|s| *s == under).unwrap_or(false))
                .cloned();

            if let Some(a) = active {
                match a
                    .user_data()
                    .get::<RefCell<WindowUserData>>()
                    .unwrap()
                    .borrow()
                    .mode
                {
                    WindowMode::Floating => {
                        ws.space.raise_element(&a, false);
                    }
                    _ => {}
                }
                ws.active_window = Some(a.clone());
            }
            self.set_keyboard_focus(Some(under));
        }
    }

    pub fn refresh_layout(&mut self) {
        let ws = self.workspaces.get_current_mut();
        ws.space.refresh();
        let fullscreen = is_fullscreen(ws.space.elements()).cloned();
        let offset = self.config.border.gap + self.config.border.thickness;

        let output_geometry = ws.space.outputs().next().and_then(|o| {
            let geo = ws.space.output_geometry(&o)?;
            let map = layer_map_for_output(&o);
            let zone = map.non_exclusive_zone();
            Some(Rectangle::new(geo.loc + zone.loc, zone.size))
        });
        let geo = match output_geometry {
            Some(g) => g,
            None => return,
        };

        let mut tiled_windows: Vec<Window> = ws
            .space
            .elements()
            .filter(|w| {
                w.user_data()
                    .get::<RefCell<WindowUserData>>()
                    .map(|d| d.borrow().mode == WindowMode::Tiled)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        tiled_windows.sort_by(|a, b| {
            let geo_a = ws.space.element_geometry(a).unwrap_or_default();
            let geo_b = ws.space.element_geometry(b).unwrap_or_default();

            // Primary sort by Y coordinate (top to bottom)
            geo_a
                .loc
                .y
                .cmp(&geo_b.loc.y)
                // Secondary sort by X coordinate (left to right)
                .then(geo_a.loc.x.cmp(&geo_b.loc.x))
        });

        let count = tiled_windows.len() as i32;

        if count == 0 {
            return;
        }

        let mut active = None;
        for elem in ws.layout.placement(tiled_windows.iter(), geo) {
            if let Some(ref full) = fullscreen {
                if full == elem.window {
                    continue;
                }
            }
            let geometry: Rectangle<i32, Logical> = Rectangle::new(
                (elem.geometry.loc.x + offset, elem.geometry.loc.y + offset).into(),
                (
                    elem.geometry.size.w - offset * 2,
                    elem.geometry.size.h - offset * 2,
                )
                    .into(),
            );
            match elem.window.underlying_surface() {
                WindowSurface::Wayland(xdg) => {
                    xdg.with_pending_state(|state| {
                        state.size = Some(geometry.size);
                    });
                    xdg.send_configure();
                    ws.space
                        .map_element(elem.window.clone(), geometry.loc, false);
                }
                #[cfg(feature = "xwayland")]
                WindowSurface::X11(x11) => {
                    x11.configure(geometry).unwrap();
                    ws.space
                        .map_element(elem.window.clone(), geometry.loc, false);
                }
            }
            if elem.geometry.to_f64().contains(self.pointer_location) {
                active = Some(elem.window.clone())
            }
        }

        let floating_windows: Vec<Window> = ws
            .space
            .elements()
            .filter(|w| {
                w.user_data()
                    .get::<RefCell<WindowUserData>>()
                    .map(|d| d.borrow().mode == WindowMode::Floating)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        for window in floating_windows {
            if let Some(geometry) = ws.space.element_geometry(&window) {
                ws.space.map_element(window.clone(), geometry.loc, false);
                if geometry.to_f64().contains(self.pointer_location) {
                    active = Some(window)
                }
            } else {
            }
        }

        if ws.active_window.is_none() {
            ws.active_window = active.clone();
            self.set_keyboard_focus(
                active.and_then(|w| w.wl_surface().map(|s| s.as_ref().clone())),
            );
        }
    }
    pub fn window_contains_pointer(
        &self,
        window: &Window,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<i32, Logical>,
    )> {
        let offset = self.config.border.thickness + self.config.border.gap;
        let ws = self.workspaces.get_current();
        let geo = ws.space.element_geometry(window)?;

        let mut offset_geo = geo;
        offset_geo.size += (offset * 2, offset * 2).into();
        offset_geo.loc -= (offset, offset).into();

        if offset_geo.contains(self.pointer_location.to_i32_round()) {
            return window
                .wl_surface()
                .map(|s| (s.as_ref().clone(), geo.loc - window.geometry().loc));
        }

        None
    }
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.workspaces
            .get_current()
            .space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
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
