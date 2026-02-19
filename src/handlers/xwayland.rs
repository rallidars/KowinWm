use std::{cell::RefCell, time::Duration};

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::xwayland::xwm::ResizeEdge as X11ResizeEdge;
use smithay::{
    delegate_xwayland_keyboard_grab, delegate_xwayland_shell,
    desktop::Window,
    input::pointer::Focus,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Point, Size},
    wayland::{
        seat::WaylandFocus,
        selection::{
            data_device::{
                clear_data_device_selection, current_data_device_selection_userdata,
                request_data_device_client_selection, set_data_device_selection,
            },
            primary_selection::{
                clear_primary_selection, current_primary_selection_userdata,
                request_primary_client_selection, set_primary_selection,
            },
            SelectionTarget,
        },
        xwayland_keyboard_grab::XWaylandKeyboardGrabHandler,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{X11Wm, XWaylandEvent, XwmHandler},
};

use crate::utils::workspaces::{WindowMode, WindowUserData};
use crate::{
    state::State,
    udev::FALLBACK_CURSOR_DATA,
    utils::grab::{MovePointerGrab, ResizePointerGrub},
    SERIAL_COUNTER,
};
impl State {
    pub fn start_xwayland(&mut self) {
        use smithay::wayland::compositor::CompositorHandler;
        use smithay::xwayland::XWayland;
        use std::process::Stdio;

        let (xwayland, client) = XWayland::spawn(
            &self.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| (),
        )
        .expect("Failed to run Xwayland");
        // SAFETY: All set_vars occur on the event loop thread
        unsafe {
            std::env::set_var("DISPLAY", format!(":{}", xwayland.display_number()));
        }

        let ret = self
            .loop_handle
            .insert_source(xwayland, move |event, _, data| {
                tracing::info!("enter xwayland event");

                match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => {
                        tracing::warn!("XWayland startuped");
                        let xwayland_scale = std::env::var("KOVINWM_XWAYLAND_SCALE")
                            .ok()
                            .and_then(|s| s.parse::<f64>().ok())
                            .unwrap_or(1.);
                        data.client_compositor_state(&client)
                            .set_client_scale(xwayland_scale);
                        let mut wm =
                            X11Wm::start_wm(data.loop_handle.clone(), x11_socket, client.clone())
                                .expect("Failed to attach X11 Window Manager");

                        wm.set_cursor(
                            &FALLBACK_CURSOR_DATA,
                            Size::from((64 as u16, 64 as u16)),
                            Point::from((1 as u16, 1 as u16)),
                        )
                        .expect("Failed to set xwayland default cursor");

                        data.xwm = Some(wm);
                        data.xdisplay = Some(display_number);
                    }
                    XWaylandEvent::Error => {
                        tracing::warn!("XWayland crashed on startup");
                    }
                }
            });
        if let Err(e) = ret {
            tracing::error!(
                "Failed to insert the XWaylandSource into the event loop: {}",
                e
            );
        }
    }
}

fn x11_resize_edge_to_xdg(edge: X11ResizeEdge) -> ResizeEdge {
    match edge {
        X11ResizeEdge::Top => ResizeEdge::Top,
        X11ResizeEdge::Left => ResizeEdge::Left,
        X11ResizeEdge::Right => ResizeEdge::Right,
        X11ResizeEdge::Bottom => ResizeEdge::Bottom,
        X11ResizeEdge::TopLeft => ResizeEdge::TopLeft,
        X11ResizeEdge::TopRight => ResizeEdge::TopRight,
        X11ResizeEdge::BottomLeft => ResizeEdge::BottomLeft,
        X11ResizeEdge::BottomRight => ResizeEdge::BottomRight,
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

delegate_xwayland_shell!(State);

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: smithay::xwayland::xwm::XwmId) -> &mut smithay::xwayland::X11Wm {
        self.xwm.as_mut().unwrap()
    }

    fn new_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }
    fn new_override_redirect_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }

    fn fullscreen_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let ws = self.workspaces.get_current_mut();

        let elem = match ws
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if w == &window))
        {
            Some(e) => e,
            None => return,
        };
        let outputs_for_window = ws.space.outputs_for_element(elem);
        let output = outputs_for_window
            .first()
            // The window hasn't been mapped yet, use the primary output instead
            .or_else(|| ws.space.outputs().next())
            // Assumes that at least one output exists
            .expect("No outputs found");
        let geometry = ws.space.output_geometry(output).unwrap();
        window.set_fullscreen(true).unwrap();
        ws.full_geo = Some(window.geometry());
        window.configure(geometry).unwrap();
        ws.space.map_element(elem.clone(), geometry.loc, false);
    }

    fn unfullscreen_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let ws = self.workspaces.get_current_mut();

        let elem = match ws
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if w == &window))
        {
            Some(e) => e,
            None => return,
        };
        window.set_fullscreen(false).unwrap();
        window.configure(ws.full_geo).unwrap();
        ws.space
            .map_element(elem.clone(), ws.full_geo.unwrap().loc, false);
        ws.full_geo.take();
    }

    fn map_window_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        window.set_mapped(true).unwrap();
        let window = Window::new_x11_window(window);
        window.user_data().insert_if_missing(|| {
            RefCell::new(WindowUserData {
                mode: WindowMode::Floating,
            })
        });
        self.workspaces.insert_window(window.clone());
        let bbox = self
            .workspaces
            .get_current_mut()
            .space
            .element_bbox(&window)
            .unwrap();
        let Some(xsurface) = window.x11_surface() else {
            unreachable!()
        };
        xsurface.configure(Some(bbox)).unwrap();
        self.refresh_layout();
        tracing::info!("map_window_xwayland");
    }
    fn mapped_override_redirect_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let location = window.geometry().loc;
        tracing::info!("mapped_over: {:?}", location);
        let window = Window::new_x11_window(window);
        self.workspaces
            .get_current_mut()
            .space
            .map_element(window.clone(), location, true);
    }

    fn destroyed_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
    }

    fn configure_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<smithay::xwayland::xwm::Reorder>,
    ) {
        // we just set the new size, but don't let windows move themselves around freely
        let mut geo = window.geometry();
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        _above: Option<smithay::xwayland::xwm::X11Window>,
    ) {
        tracing::info!("configure_window_xwayland");
        let Some(elem) = self
            .workspaces
            .get_current()
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if w == &window))
            .cloned()
        else {
            return;
        };
        self.workspaces
            .get_current_mut()
            .space
            .map_element(elem, geometry.loc, false);

        // TODO: We don't properly handle the order of override-redirect windows here,
        //       they are always mapped top and then never reordered.
    }

    fn move_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        _button: u32,
    ) {
        let Some(start_data) = self.pointer.grab_start_data() else {
            return;
        };

        let ws = self.workspaces.get_current();
        let Some(element) = ws
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if *w == window))
        else {
            return;
        };

        let start_loc = ws.space.element_location(element).unwrap();
        let grab = MovePointerGrab {
            start_data,
            window: element.clone(),
            start_loc,
        };

        let pointer = self.pointer.clone();
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }
    fn resize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
        _button: u32,
        resize_edge: smithay::xwayland::xwm::ResizeEdge,
    ) {
        let ws = self.workspaces.get_current();
        let Some(element) = ws
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if w == &window))
        else {
            return;
        };

        let pointer = self.seat.get_pointer().unwrap();
        let start_data = pointer.grab_start_data().unwrap();

        let window_geo = match ws.space.element_geometry(&element) {
            Some(l) => l,
            None => return,
        };

        let grab = ResizePointerGrub {
            start_data,
            window: element.clone(),
            edges: x11_resize_edge_to_xdg(resize_edge),
            start_geo: window_geo,
            last_window_size: window_geo.size,
        };
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }
    fn unmapped_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        let ws = self.workspaces.get_current_mut();
        let maybe = ws
            .space
            .elements()
            .find(|e| matches!(e.x11_surface(), Some(w) if w == &window))
            .cloned();
        if let Some(elem) = maybe {
            self.workspaces.remove_window(&elem);
        }
        if !window.is_override_redirect() {
            window.set_mapped(false).unwrap();
        }
        tracing::info!("unmapped")
    }

    fn allow_selection_access(
        &mut self,
        xwm: smithay::xwayland::xwm::XwmId,
        _selection: SelectionTarget,
    ) -> bool {
        if let Some(keyboard) = self.seat.get_keyboard() {
            // check that an X11 window is focused
            if let Some(surface) = keyboard.current_focus() {
                if let Some(window) = self.window_for_surface(&surface) {
                    if let Some(surface) = window.x11_surface() {
                        if surface.xwm_id().unwrap() == xwm {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    fn send_selection(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        selection: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::unix::prelude::OwnedFd,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if let Err(err) = request_data_device_client_selection(&self.seat, mime_type, fd) {
                    tracing::error!(
                        ?err,
                        "Failed to request current wayland clipboard for Xwayland",
                    );
                }
            }
            SelectionTarget::Primary => {
                if let Err(err) = request_primary_client_selection(&self.seat, mime_type, fd) {
                    tracing::error!(
                        ?err,
                        "Failed to request current wayland primary selection for Xwayland",
                    );
                }
            }
        }
    }

    fn new_selection(
        &mut self,
        xwm: smithay::xwayland::xwm::XwmId,
        selection: SelectionTarget,
        mime_types: Vec<String>,
    ) {
        tracing::trace!(?selection, ?mime_types, "Got Selection from X11",);
        // TODO check, that focused windows is X11 window before doing this
        match selection {
            SelectionTarget::Clipboard => {
                set_data_device_selection(&self.display_handle, &self.seat, mime_types, ())
            }
            SelectionTarget::Primary => {
                set_primary_selection(&self.display_handle, &self.seat, mime_types, ())
            }
        }
    }

    fn cleared_selection(
        &mut self,
        xwm: smithay::xwayland::xwm::XwmId,
        selection: SelectionTarget,
    ) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat)
                }
            }
        }
    }
    fn disconnected(&mut self, _xwm: smithay::xwayland::xwm::XwmId) {
        self.xwm = None
    }
}

impl XWaylandKeyboardGrabHandler for State {
    fn keyboard_focus_for_xsurface(&self, surface: &WlSurface) -> Option<WlSurface> {
        let ws = self.workspaces.get_current();
        let elem = ws
            .space
            .elements()
            .find(|elem| elem.wl_surface().as_deref() == Some(surface));
        elem.map(|_| surface.clone())
    }
}
delegate_xwayland_keyboard_grab!(State);
