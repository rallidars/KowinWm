use smithay::{
    backend::renderer::element::Kind,
    desktop::{layer_map_for_output, space::SpaceElement, Space, Window},
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point},
    wayland::{seat::WaylandFocus, shell::xdg::ToplevelSurface},
};

use crate::{action::Direction, config::Config, render::CustomRenderElements};

pub struct Workspace {
    pub space: Space<Window>,
    pub active_window: Option<Window>,
    pub prev_window: Option<Window>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            space: Space::default(),
            active_window: None,
            prev_window: None,
        }
    }
}

pub struct Workspaces {
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub prev_workspace: usize,
}

impl Workspaces {
    pub fn new() -> Self {
        Self {
            workspaces: (0..=4).map(|_| Workspace::new()).collect(),
            active_workspace: 0,
            prev_workspace: 0,
        }
    }

    pub fn set_active_window(&mut self, window: Option<Window>) {
        self.get_current_mut().active_window = window
    }

    pub fn get_active_window(&self) -> Option<Window> {
        self.get_current().active_window.clone()
    }

    pub fn get_current_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace]
    }

    pub fn get_current(&self) -> &Workspace {
        &self.workspaces[self.active_workspace]
    }

    pub fn active_ws(&self) -> usize {
        return self.active_workspace;
    }

    pub fn is_ws_empty(&self, workspace: usize) -> bool {
        return self.workspaces[workspace].space.elements().len() == 0;
    }

    pub fn set_active_workspace(&mut self, workspace: usize) {
        if workspace >= self.workspaces.len() {
            return;
        }
        self.prev_workspace = self.active_workspace;
        self.active_workspace = workspace;
        self.layout();
    }

    pub fn move_window_to_ws(&mut self, ws_index: usize) {
        let active = match self.get_active_window() {
            Some(index) => index,
            None => return,
        };

        self.get_current_mut().space.unmap_elem(&active);
        self.set_active_workspace(ws_index);
        self.insert_window(active.clone());
    }

    pub fn remove_window(&mut self, surface: &Window) {
        self.get_current_mut().space.unmap_elem(surface);
        self.layout();
    }

    pub fn insert_window(&mut self, window: Window) {
        self.get_current_mut()
            .space
            .map_element(window.clone(), (0, 0), false);
        self.layout();
    }

    pub fn change_focus(&mut self, direction: &Direction, loc: &mut Point<f64, Logical>) {
        let space = &self.get_current().space;

        let focused = self.get_active_window();
        if let Some((window, _)) = best_window(direction, space, focused) {
            *loc = window_center(&space, &window).unwrap();
        }
    }

    pub fn move_window(&mut self, direction: &Direction, loc: &mut Point<f64, Logical>) {
        let space = &self.get_current().space;
        let Some(focused) = self.get_active_window() else {
            return;
        };

        // Find the best window to swap with
        let Some((best, _)) = best_window(direction, space, Some(focused.clone())) else {
            return;
        };

        // Avoid pointless work if same window
        if best == focused {
            return;
        }

        let mut windows = space.elements().map(|w| w.clone()).collect::<Vec<Window>>();
        let focused_pos = match windows.iter().position(|w| w == &focused) {
            Some(pos) => pos,
            None => return,
        };
        let best_pos = match windows.iter().position(|w| w == &best) {
            Some(w) => w,
            None => return,
        };
        windows.swap(focused_pos, best_pos);
        let space = &mut self.get_current_mut().space;
        *loc = window_center(&space, &best).unwrap();
        for window in windows {
            space.map_element(window, (0, 0), false);
        }

        self.layout();
    }

    pub fn layout(&mut self) {
        let gap = 2;
        let space = &mut self.get_current_mut().space;
        space.refresh();

        let output = match space.outputs().next() {
            Some(o) => o.clone(),
            None => return, // no output, nothing to do
        };
        let geo = layer_map_for_output(&output).non_exclusive_zone();

        let output_width = geo.size.w;
        let output_height = geo.size.h;

        let elements: Vec<_> = space.elements().map(|e| e.clone()).collect();
        let count = elements.len() as i32;

        if count == 0 {
            return;
        }

        if let Some(_fs) = is_fullscreen(elements.iter()) {
            return;
        }

        let half_width = output_width / 2;
        let vertical_height = if count > 1 {
            output_height / (count - 1)
        } else {
            output_height
        };

        for (i, window) in elements.into_iter().enumerate() {
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

            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some((width - gap, height - gap).into());
                });
                toplevel.send_pending_configure();
            }

            space.map_element(window, (x, y), false);
        }
    }
}

pub fn is_fullscreen<'a, I>(elements: I) -> Option<&'a Window>
where
    I: Iterator<Item = &'a Window>,
{
    for element in elements {
        if element
            .toplevel()
            .unwrap()
            .current_state()
            .states
            .contains(xdg_toplevel::State::Fullscreen)
        {
            return Some(element);
        }
    }
    None
}

pub fn window_center(space: &Space<Window>, window: &Window) -> Option<Point<f64, Logical>> {
    let geo = space.element_geometry(window)?;

    Some(Point::from((
        (geo.loc.x + geo.size.w / 2) as f64,
        (geo.loc.y + geo.size.h / 2) as f64,
    )))
}
pub fn best_window(
    direction: &Direction,
    space: &Space<Window>,
    focused: Option<Window>,
) -> Option<(Window, i32)> {
    let focused = focused?;
    let focused_geo = space.element_geometry(&focused)?;

    let focused_center: Point<i32, Kind> = Point::from((
        focused_geo.loc.x + focused_geo.size.w / 2,
        focused_geo.loc.y + focused_geo.size.h / 2,
    ));

    let mut best: Option<(Window, i32)> = None;

    for window in space.elements() {
        if window == &focused {
            continue;
        }

        let geo = match space.element_geometry(window) {
            Some(g) => g,
            None => continue,
        };

        let center: Point<i32, Kind> =
            Point::from((geo.loc.x + geo.size.w / 2, geo.loc.y + geo.size.h / 2));

        let dx = center.x - focused_center.x;
        let dy = center.y - focused_center.y;

        let valid = match direction {
            Direction::Left => dx < 0 && dy.abs() < geo.size.h,
            Direction::Right => dx > 0 && dy.abs() < geo.size.h,
            Direction::Top => dy < 0 && dx.abs() < geo.size.w,
            Direction::Down => dy > 0 && dx.abs() < geo.size.w,
        };

        if !valid {
            continue;
        }

        let distance = dx.abs() + dy.abs();

        match best {
            None => best = Some((window.clone(), distance)),
            Some((_, best_dist)) if distance < best_dist => best = Some((window.clone(), distance)),
            _ => {}
        }
    }
    best
}
