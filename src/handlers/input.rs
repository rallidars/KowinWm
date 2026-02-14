use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::{layer_map_for_output, WindowSurfaceType},
    input::{
        keyboard::{
            keysyms::{self, KEY_XF86Switch_VT_1, KEY_XF86Switch_VT_12},
            FilterResult,
        },
        pointer::{AxisFrame, ButtonEvent, GrabStartData, MotionEvent, RelativeMotionEvent},
    },
    utils::{Logical, Point},
    wayland::{
        seat::WaylandFocus,
        shell::{wlr_layer, xdg::XdgShellHandler},
    },
};

use crate::{state::State, SERIAL_COUNTER};
use crate::{
    utils::action::{Action, Direction},
    utils::config::parse_keybind,
};

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => {
                let press_state = event.state();
                let action = self.seat.get_keyboard().unwrap().input::<Action, _>(
                    self,
                    event.key_code(),
                    press_state,
                    0.into(),
                    0,
                    |state, modifiers, handle| {
                        // Get representation of what key was pressed.
                        if press_state == KeyState::Pressed {
                            let keysym = handle.modified_sym();
                            for (keymap, action) in &state.config.keymaps {
                                if let Some((config_modifiers, config_keysyms)) =
                                    parse_keybind(&keymap)
                                {
                                    if modifiers.logo == config_modifiers.logo
                                        && modifiers.shift == config_modifiers.shift
                                        && modifiers.ctrl == config_modifiers.ctrl
                                        && modifiers.alt == config_modifiers.alt
                                        && keysym.raw() == config_keysyms
                                    {
                                        return FilterResult::Intercept(action.clone());
                                    }
                                }
                            }
                            if (KEY_XF86Switch_VT_1..=KEY_XF86Switch_VT_12)
                                .contains(&handle.modified_sym().raw())
                            {
                                // VTSwitch
                                let vt =
                                    (handle.modified_sym().raw() - KEY_XF86Switch_VT_1 + 1) as i32;
                                return FilterResult::Intercept(Action::VTSwitch(vt));
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
                    self.set_keyboard_focus_auto();

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
                    self.set_keyboard_focus_auto();

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
                let pointer = self.seat.get_pointer().unwrap();
                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();
                let button_state = event.state();

                self.set_keyboard_focus_auto();

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event } => {
                let horizontal_amount = event
                    .amount(Axis::Horizontal)
                    .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 3.0);
                let vertical_amount = event
                    .amount(Axis::Vertical)
                    .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 3.0);
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

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
}
