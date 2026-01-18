use std::time::Duration;

use smithay::{
    delegate_xwayland_keyboard_grab, delegate_xwayland_shell,
    desktop::Window,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Point, Size},
    wayland::{
        seat::WaylandFocus,
        xwayland_keyboard_grab::XWaylandKeyboardGrabHandler,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    },
    xwayland::{X11Wm, XWaylandEvent, XwmHandler},
};

use crate::{state::State, udev::FALLBACK_CURSOR_DATA};
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

    fn map_window_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        window: smithay::xwayland::X11Surface,
    ) {
        window.set_mapped(true).unwrap();
        let window = Window::new_x11_window(window);
        self.workspaces.insert_window(window);
    }
    fn mapped_override_redirect_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }

    fn destroyed_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
    }

    fn configure_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        _w: Option<u32>,
        _h: Option<u32>,
        _reorder: Option<smithay::xwayland::xwm::Reorder>,
    ) {
    }
    fn configure_notify(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
        _geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        _above: Option<smithay::xwayland::xwm::X11Window>,
    ) {
    }

    fn move_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
        _button: u32,
    ) {
    }
    fn resize_request(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
        _button: u32,
        _resize_edge: smithay::xwayland::xwm::ResizeEdge,
    ) {
    }
    fn unmapped_window(
        &mut self,
        _xwm: smithay::xwayland::xwm::XwmId,
        _window: smithay::xwayland::X11Surface,
    ) {
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
