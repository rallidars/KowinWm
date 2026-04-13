use std::cell::RefCell;

use smithay::{
    desktop::{Window, WindowSurface},
    input::pointer::{
        AxisFrame, ButtonEvent, Focus, GestureHoldBeginEvent, GestureHoldEndEvent,
        GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
        GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData,
        MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel::{self, ResizeEdge},
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle, Serial, Size},
    wayland::seat::WaylandFocus,
};

use crate::{state::State, utils::workspaces::WindowMode};

pub struct MovePointerGrab {
    pub start_data: GrabStartData<State>,
    pub window: Window,
    pub start_loc: Point<i32, Logical>,
}

impl State {
    pub fn init_pointer_move_grab(&mut self, button: u32, serial: Serial) {
        let surface = match self.surface_under() {
            Some(surface) => surface,
            None => return,
        };
        let ws = self.workspaces.get_current_mut();
        let window = ws
            .space
            .elements()
            .find(|element| {
                element
                    .wl_surface()
                    .map(|s| &*s == &surface.0)
                    .unwrap_or(false)
            })
            .unwrap()
            .clone();

        tracing::info!("start reposition");
        let start_data = GrabStartData {
            focus: Some(surface),
            button: 272,
            location: self.pointer_location,
        };
        let window_geo = match ws.space.element_geometry(&window) {
            Some(l) => l,
            None => return,
        };

        //let pointer_pos = start_data.location;

        //let start_loc: Point<i32, Logical> = (
        //    pointer_pos.x as i32 - (window_geo.size.w as i32 / 2),
        //    pointer_pos.y as i32 - (window_geo.size.h as i32 / 2),
        //)
        //    .into();

        //ws.space.map_element(window.clone(), start_loc, false);

        let grab = MovePointerGrab {
            start_data,
            window: window.clone(),
            start_loc: window_geo.loc,
        };

        let pointer = self.seat.get_pointer().unwrap();
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

impl PointerGrab<State> for MovePointerGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        if let Some(window_data) = self.window.user_data().get::<RefCell<WindowMode>>() {
            match *window_data.borrow() {
                WindowMode::Tiled => {}
                WindowMode::Floating => {
                    let ws = data.workspaces.get_current_mut();
                    let delta = event.location - self.start_data.location;
                    let new_location = self.start_loc.to_f64() + delta;
                    ws.space
                        .map_element(self.window.clone(), new_location.to_i32_round(), false);
                }
                _ => {}
            }
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            if !self.window.alive() {
                return;
            }
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut State) {}
}

/// Information about the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ResizeData {
    /// The edges the surface is being resized with.
    pub edges: ResizeEdge,
    /// The initial window location.
    pub initial_window_location: Point<i32, Logical>,
    /// The initial window size (geometry width and height).
    pub initial_window_size: Size<i32, Logical>,
}

/// State of the resize operation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ResizeState {
    /// The surface is not being resized.
    #[default]
    NotResizing,
    /// The surface is currently being resized.
    Resizing(ResizeData),
    /// The resize has finished, and the surface needs to ack the final configure.
    WaitingForFinalAck(ResizeData, Serial),
    /// The resize has finished, and the surface needs to commit its final state.
    WaitingForCommit(ResizeData),
}

pub struct ResizePointerGrub {
    pub start_data: GrabStartData<State>,
    pub window: Window,
    pub edges: ResizeEdge,
    pub start_geo: Rectangle<i32, Logical>,
    pub last_window_size: Size<i32, Logical>,
}

pub fn detect_resize_edge(
    geo: Rectangle<i32, Logical>,
    pointer: Point<f64, Logical>,
    border: i32,
    outer: (i32, i32),
) -> Option<ResizeEdge> {
    let px = pointer.x as i32;
    let py = pointer.y as i32;

    // Detect edges with outer zone included
    let left = px <= geo.loc.x + outer.0 && px >= geo.loc.x - border;
    let right = px >= geo.loc.x + geo.size.w - outer.0 && px <= geo.loc.x + geo.size.w + border;
    let top = py <= geo.loc.y + outer.1 && py >= geo.loc.y - border;
    let bottom = py >= geo.loc.y + geo.size.h - outer.1 && py <= geo.loc.y + geo.size.h + border;

    match (left, right, top, bottom) {
        (true, false, true, false) => Some(ResizeEdge::TopLeft),
        (true, false, false, true) => Some(ResizeEdge::BottomLeft),
        (false, true, true, false) => Some(ResizeEdge::TopRight),
        (false, true, false, true) => Some(ResizeEdge::BottomRight),
        (true, false, false, false) => Some(ResizeEdge::Left),
        (false, true, false, false) => Some(ResizeEdge::Right),
        (false, false, true, false) => Some(ResizeEdge::Top),
        (false, false, false, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

impl State {
    pub fn init_pointer_resize_grab(&mut self, button: u32, serial: Serial) {
        let surface = match self.surface_under() {
            Some(surface) => surface,
            None => return,
        };
        let ws = self.workspaces.get_current_mut();
        let window = match ws.space.elements().find(|element| {
            element
                .wl_surface()
                .map(|s| &*s == &surface.0)
                .unwrap_or(false)
        }) {
            Some(w) => w.clone(),
            None => return,
        };

        let start_data = GrabStartData {
            focus: Some(surface),
            button: button,
            location: self.pointer_location,
        };

        let window_geo = match self
            .workspaces
            .get_current()
            .space
            .element_geometry(&window)
        {
            Some(l) => l,
            None => return,
        };

        let outer = if button == 272 {
            (10, 10)
        } else {
            (window_geo.size.w / 2, window_geo.size.h / 2)
        };

        if let Some(edges) = detect_resize_edge(
            window_geo,
            self.pointer_location,
            self.config.border.thickness,
            outer,
        ) {
            tracing::info!("start resize");

            let grab = ResizePointerGrub {
                start_data,
                window: window.clone(),
                edges,
                start_geo: window_geo,
                last_window_size: window_geo.size,
            };
            self.seat
                .get_pointer()
                .unwrap()
                .set_grab(self, grab, serial, Focus::Clear);
        }
    }
}

impl PointerGrab<State> for ResizePointerGrub {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the grab is active, no client has pointer focus
        handle.motion(data, None, event);

        // It is impossible to get `min_size` and `max_size` of dead toplevel, so we return early.
        if !self.window.alive() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            return;
        }
        let mode = match self.window.user_data().get::<RefCell<WindowMode>>() {
            Some(m) => m.borrow().clone(),
            None => return,
        };

        match mode {
            WindowMode::Tiled => {}
            WindowMode::Floating => {
                let delta = event.location - self.start_data.location;

                let mut new_size = self.start_geo.size;
                let mut new_loc = self.start_geo.loc;

                match self.edges {
                    ResizeEdge::Left => {
                        let dx = delta.x as i32;
                        new_size.w -= dx;
                        new_loc.x += dx;
                    }
                    ResizeEdge::Top => {
                        let dy = delta.y as i32;
                        new_size.h -= dy;
                        new_loc.y += dy;
                    }
                    ResizeEdge::Right => {
                        new_size.w += delta.x as i32;
                    }
                    ResizeEdge::Bottom => {
                        new_size.h += delta.y as i32;
                    }
                    ResizeEdge::TopRight => {
                        let dy = delta.y as i32;
                        new_size.h -= dy;
                        new_loc.y += dy;
                        new_size.w += delta.x as i32;
                    }
                    ResizeEdge::TopLeft => {
                        let dy = delta.y as i32;
                        new_size.h -= dy;
                        new_loc.y += dy;
                        let dx = delta.x as i32;
                        new_size.w -= dx;
                        new_loc.x += dx;
                    }
                    ResizeEdge::BottomLeft => {
                        new_size.h += delta.y as i32;
                        let dx = delta.x as i32;
                        new_size.w -= dx;
                        new_loc.x += dx;
                    }
                    ResizeEdge::BottomRight => {
                        new_size.h += delta.y as i32;
                        new_size.w += delta.x as i32;
                    }
                    _ => {}
                }

                // Prevent zero / negative sizes
                new_size.w = new_size.w.max(100);
                new_size.h = new_size.h.max(100);
                let ws = data.workspaces.get_current_mut();

                match self.edges {
                    ResizeEdge::Left
                    | ResizeEdge::Top
                    | ResizeEdge::TopLeft
                    | ResizeEdge::BottomLeft => {
                        ws.space.map_element(self.window.clone(), new_loc, false);
                    }
                    _ => {}
                }
                let rec = Rectangle::new(new_loc, new_size);

                // Send configure to client
                match self.window.underlying_surface() {
                    WindowSurface::Wayland(xdg) => {
                        xdg.with_pending_state(|state| {
                            state.states.set(xdg_toplevel::State::Resizing);
                            state.size = Some(rec.size);
                        });

                        self.window.toplevel().unwrap().send_configure();
                    }
                    #[cfg(feature = "xwayland")]
                    WindowSurface::X11(x11) => {
                        x11.configure(Rectangle::new(new_loc, new_size)).unwrap();
                    }
                }
                if !rec.contains(event.location.to_i32_round()) {
                    handle.unset_grab(self, data, event.serial, event.time, true);
                }

                self.last_window_size = new_size;
            }
            _ => {}
        }
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
            if !self.window.alive() {
                return;
            }
        }
    }

    fn axis(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut State) {}
}
