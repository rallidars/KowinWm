use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, DeviceCapability, Event, GestureBeginEvent,
            GestureEndEvent, GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _,
            InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent,
            PointerButtonEvent, PointerMotionEvent, TouchEvent,
        },
        libinput::LibinputInputBackend,
    },
    desktop::layer_map_for_output,
    input::{
        keyboard::{
            keysyms::{KEY_XF86Switch_VT_1, KEY_XF86Switch_VT_12},
            FilterResult, Keysym,
        },
        pointer::{
            AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent,
            GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent,
            GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent,
            RelativeMotionEvent,
        },
        touch::{DownEvent, UpEvent},
    },
    reexports::wayland_server::protocol::wl_pointer,
    utils::{Logical, Point},
    wayland::{
        compositor,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat,
        seat::WaylandFocus,
        shell::wlr_layer::{self, KeyboardInteractivity, LayerSurfaceCachedState},
        tablet_manager::{TabletDescriptor, TabletSeatTrait},
    },
};

use crate::{handlers::input, state::State, SERIAL_COUNTER};
use crate::{utils::action::Action, utils::config::parse_keybind};

impl State {
    pub fn process_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        match event {
            InputEvent::Keyboard { event } => {
                let keycode = event.key_code();
                let state = event.state();
                tracing::debug!(?keycode, ?state, "key");
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let press_state = event.state();
                let keyboard = self.seat.get_keyboard().unwrap();

                for layer in self.layer_shell_state.layer_surfaces().rev() {
                    let exclusive = compositor::with_states(&layer.wl_surface(), |states| {
                        let mut guard = states.cached_state.get::<LayerSurfaceCachedState>();
                        let data = guard.current();
                        data.keyboard_interactivity == KeyboardInteractivity::Exclusive
                            && (data.layer == wlr_layer::Layer::Top
                                || data.layer == wlr_layer::Layer::Overlay)
                    });
                    if exclusive {
                        let surface = self.workspaces.get_current().space.outputs().find_map(|o| {
                            let map = layer_map_for_output(o);
                            let cloned =
                                map.layers().find(|l| l.layer_surface() == &layer).cloned();
                            cloned
                        });
                        if let Some(surface) = surface {
                            keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
                            keyboard.input::<(), _>(
                                self,
                                keycode,
                                state,
                                serial,
                                time,
                                |_, _, _| FilterResult::Forward,
                            );
                            return;
                        };
                    }
                }

                let inhibited = self
                    .workspaces
                    .get_current()
                    .space
                    .element_under(self.pointer.current_location())
                    .and_then(|(window, _)| {
                        let surface = window.wl_surface()?;
                        self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface)
                    })
                    .map(|inhibitor| inhibitor.is_active())
                    .unwrap_or(false);

                let action = self.seat.get_keyboard().unwrap().input::<Action, _>(
                    self,
                    keycode,
                    press_state,
                    0.into(),
                    0,
                    |state, modifiers, handle| {
                        // Get representation of what key was pressed.
                        if press_state == KeyState::Pressed {
                            if !inhibited {
                                let raw_syms = {
                                    let xkb = handle.xkb().lock().unwrap();
                                    let mut raws = Vec::<Keysym>::new();
                                    for layout in xkb.layouts() {
                                        raws.extend(xkb.raw_syms_for_key_in_layout(keycode, layout))
                                    }
                                    raws
                                };

                                for (keymap, action) in &state.config.keymaps {
                                    if let Some((config_modifiers, config_keysyms)) =
                                        parse_keybind(&keymap)
                                    {
                                        if (modifiers.logo == config_modifiers.logo
                                            && modifiers.shift == config_modifiers.shift
                                            && modifiers.ctrl == config_modifiers.ctrl
                                            && modifiers.alt == config_modifiers.alt)
                                            && raw_syms.contains(&config_keysyms)
                                        {
                                            return FilterResult::Intercept(action.clone());
                                        }
                                    }
                                }
                                if (KEY_XF86Switch_VT_1..=KEY_XF86Switch_VT_12)
                                    .contains(&handle.modified_sym().raw())
                                {
                                    // VTSwitch
                                    let vt = (handle.modified_sym().raw() - KEY_XF86Switch_VT_1 + 1)
                                        as i32;
                                    return FilterResult::Intercept(Action::VTSwitch(vt));
                                }
                            }
                        }
                        FilterResult::Forward
                    },
                );
                if let Some(action) = action {
                    action.execute(self);
                }
            }

            InputEvent::PointerMotionAbsolute { event } => {
                let ws = self.workspaces.get_current();
                let output = ws.space.outputs().next().unwrap().clone();

                let output_geo = ws.space.output_geometry(&output).unwrap();

                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();

                let serial = SERIAL_COUNTER.next_serial();

                if let Some(ptr) = self.seat.get_pointer() {
                    self.pointer_location = self.clamp_coords(pos);

                    let under = self.surface_under();
                    if !ptr.is_grabbed() {
                        self.set_keyboard_focus_auto();
                    }

                    ptr.motion(
                        self,
                        under, // (Option<(WlSurface, Point<f64, Logical>)>)
                        &MotionEvent {
                            location: pos,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    ptr.frame(self);
                }
            }
            InputEvent::PointerMotion { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let delta = (event.delta_x(), event.delta_y()).into();
                self.pointer_location += delta;
                self.pointer_location = self.clamp_coords(self.pointer_location);
                let under = self.surface_under();

                if let Some(ptr) = self.seat.get_pointer() {
                    if !ptr.is_grabbed() {
                        self.set_keyboard_focus_auto();
                    }

                    ptr.motion(
                        self,
                        under.clone(),
                        &MotionEvent {
                            location: self.pointer_location,
                            serial,
                            time: event.time_msec(),
                        },
                    );
                    ptr.relative_motion(
                        self,
                        under,
                        &RelativeMotionEvent {
                            delta,
                            delta_unaccel: event.delta_unaccel(),
                            utime: event.time(),
                        },
                    );
                    ptr.frame(self);
                }
            }
            InputEvent::PointerButton { event, .. } => {
                tracing::info!("Pointer button");
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();
                let state = wl_pointer::ButtonState::from(event.state());

                if state == wl_pointer::ButtonState::Pressed {
                    if button == 272 {
                        self.init_pointer_resize_grab(button, serial);
                    }
                }
                if !pointer.is_grabbed() {
                    self.set_keyboard_focus_auto();
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: state.try_into().unwrap(),
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event } => {
                let horizontal_amount =
                    event.amount(input::Axis::Horizontal).unwrap_or_else(|| {
                        event.amount_v120(input::Axis::Horizontal).unwrap_or(0.0) * 3.0
                    });
                let vertical_amount = event.amount(input::Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(input::Axis::Vertical).unwrap_or(0.0) * 3.0
                });
                let horizontal_amount_discrete = event.amount_v120(input::Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(input::Axis::Vertical);

                {
                    let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                    if horizontal_amount != 0.0 {
                        frame = frame.value(Axis::Horizontal, horizontal_amount);
                        if let Some(discrete) = horizontal_amount_discrete {
                            frame = frame.v120(Axis::Horizontal, discrete as i32);
                        }
                    } else if event.source() == AxisSource::Finger {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if vertical_amount != 0.0 {
                        frame = frame.value(Axis::Vertical, vertical_amount);
                        if let Some(discrete) = vertical_amount_discrete {
                            frame = frame.v120(Axis::Vertical, discrete as i32);
                        }
                    } else if event.source() == AxisSource::Finger {
                        frame = frame.stop(Axis::Vertical);
                    }
                    let pointer = self.seat.get_pointer().unwrap();
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }

            // Device Input
            InputEvent::DeviceAdded { mut device } => {
                if device.has_capability(DeviceCapability::TabletTool.into()) {
                    self.seat
                        .tablet_seat()
                        .add_tablet::<Self>(&self.display_handle, &TabletDescriptor::from(&device));
                }

                if device.has_capability(DeviceCapability::Touch.into())
                    && self.seat.get_touch().is_none()
                {
                    self.seat.add_touch();
                }
                device.config_tap_set_enabled(true).ok();
                device.config_tap_set_drag_enabled(true).ok();
            }

            InputEvent::DeviceRemoved { device } => {
                if device.has_capability(DeviceCapability::TabletTool.into()) {
                    let tablet_seat = self.seat.tablet_seat();

                    tablet_seat.remove_tablet(&TabletDescriptor::from(&device));

                    // If there are no tablets in seat we can remove all tools
                    if tablet_seat.count_tablets() == 0 {
                        tablet_seat.clear_tools();
                    }
                }
            }

            // Touch input
            InputEvent::TouchUp { event } => {
                let Some(touch) = self.seat.get_touch() else {
                    return;
                };

                let serial = SERIAL_COUNTER.next_serial();
                self.set_keyboard_focus_auto();

                touch.up(
                    self,
                    &UpEvent {
                        slot: event.slot(),
                        serial,
                        time: event.time_msec(),
                    },
                );
            }
            InputEvent::TouchDown { event } => {
                let Some(touch) = self.seat.get_touch() else {
                    return;
                };
                let Some(touch_location) = self.touch_location_transformed(&event) else {
                    return;
                };
                self.pointer_location = touch_location;

                let serial = SERIAL_COUNTER.next_serial();
                self.set_keyboard_focus_auto();

                let under = self.surface_under();

                touch.down(
                    self,
                    under,
                    &DownEvent {
                        slot: event.slot(),
                        location: touch_location,
                        serial,
                        time: event.time_msec(),
                    },
                );
            }

            InputEvent::TouchMotion { event } => {
                let Some(touch) = self.seat.get_touch() else {
                    return;
                };
                let Some(touch_location) = self.touch_location_transformed(&event) else {
                    return;
                };

                let under = self.surface_under();
                touch.motion(
                    self,
                    under,
                    &smithay::input::touch::MotionEvent {
                        slot: event.slot(),
                        location: touch_location,
                        time: event.time_msec(),
                    },
                );
            }
            InputEvent::TouchFrame { event } => {
                let Some(touch) = self.seat.get_touch() else {
                    return;
                };
                touch.frame(self);
            }
            InputEvent::TouchCancel { event } => {
                let Some(touch) = self.seat.get_touch() else {
                    return;
                };
                touch.cancel(self);
            }

            // GesturesInput
            InputEvent::GestureSwipeBegin { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_swipe_begin(
                    self,
                    &GestureSwipeBeginEvent {
                        serial,
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GestureSwipeUpdate { event } => {
                let pointer = self.pointer.clone();
                pointer.gesture_swipe_update(
                    self,
                    &GestureSwipeUpdateEvent {
                        time: event.time_msec(),
                        delta: event.delta(),
                    },
                );
            }
            InputEvent::GestureSwipeEnd { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_swipe_end(
                    self,
                    &GestureSwipeEndEvent {
                        serial,
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }
            InputEvent::GesturePinchBegin { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_pinch_begin(
                    self,
                    &GesturePinchBeginEvent {
                        serial,
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GesturePinchUpdate { event } => {
                let pointer = self.pointer.clone();
                pointer.gesture_pinch_update(
                    self,
                    &GesturePinchUpdateEvent {
                        time: event.time_msec(),
                        delta: event.delta(),
                        scale: event.scale(),
                        rotation: event.rotation(),
                    },
                );
            }
            InputEvent::GesturePinchEnd { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_pinch_end(
                    self,
                    &GesturePinchEndEvent {
                        serial,
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }
            InputEvent::GestureHoldBegin { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_hold_begin(
                    self,
                    &GestureHoldBeginEvent {
                        serial,
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GestureHoldEnd { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let pointer = self.pointer.clone();
                pointer.gesture_hold_end(
                    self,
                    &GestureHoldEndEvent {
                        serial,
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }

            InputEvent::TabletToolTip { event } => {}

            _ => {}
        }
    }
    fn clamp_coords(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        let ws = self.workspaces.get_current();
        let (pos_x, pos_y) = pos.into();
        let (max_x, max_y) = ws
            .space
            .output_geometry(ws.space.outputs().next().unwrap())
            .unwrap()
            .size
            .into();
        let clamped_x = pos_x.max(0.0).min(max_x as f64);
        let clamped_y = pos_y.max(0.0).min(max_y as f64);
        (clamped_x, clamped_y).into()
    }
    fn touch_location_transformed<B: InputBackend, E: AbsolutePositionEvent<B>>(
        &self,
        evt: &E,
    ) -> Option<Point<f64, Logical>> {
        let ws = self.workspaces.get_current();
        let output = ws
            .space
            .outputs()
            .find(|output| output.name().starts_with("eDP"))
            .or_else(|| ws.space.outputs().next());

        let output = output?;
        let output_geometry = ws.space.output_geometry(output)?;

        let transform = output.current_transform();
        let size = transform.invert().transform_size(output_geometry.size);
        Some(
            transform.transform_point_in(evt.position_transformed(size), &size.to_f64())
                + output_geometry.loc.to_f64(),
        )
    }
}
