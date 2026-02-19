use std::cell::RefCell;

use smithay::{
    backend::renderer::{
        element::{AsRenderElements, Kind},
        ImportAll, Renderer,
    },
    desktop::{layer_map_for_output, space::SpaceElement, Space, Window, WindowSurface},
    output::Output,
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel,
    utils::{Logical, Point, Rectangle, Size},
    wayland::{seat::WaylandFocus, shell::xdg::ToplevelSurface},
};

use crate::utils::{action::Direction, layout::LayoutState};

#[derive(PartialEq, Clone)]
pub enum WindowMode {
    Tiled,
    Floating,
    Grabed(Rectangle<i32, Logical>),
    Fullscreen(Rectangle<i32, Logical>),
}

pub struct WindowUserData {
    pub mode: WindowMode,
}

pub struct Workspace {
    pub full_geo: Option<Rectangle<i32, Logical>>,
    pub space: Space<Window>,
    pub layout: LayoutState,
    pub active_window: Option<Window>,
    pub prev_window: Option<Window>,
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            full_geo: None,
            space: Space::default(),
            layout: LayoutState::default(),
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
    pub fn new(w: u8) -> Self {
        Self {
            workspaces: (0..w).map(|_| Workspace::new()).collect(),
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
    }

    pub fn move_window_to_ws(&mut self, ws_index: usize) {
        if self.active_workspace == ws_index {
            return;
        }
        let active = match self.get_active_window() {
            Some(index) => index,
            None => return,
        };

        let ws = self.get_current_mut();
        let loc = ws.space.element_location(&active);
        ws.space.unmap_elem(&active);
        self.set_active_workspace(ws_index);
        self.insert_window(active.clone());
        if let Some(loc) = loc {
            self.get_current_mut()
                .space
                .map_element(active.clone(), loc, false);
        }
    }

    pub fn remove_window(&mut self, window: &Window) {
        let ws = self.get_current_mut();
        ws.space.unmap_elem(window);
        ws.active_window = None;
    }

    pub fn insert_window(&mut self, window: Window) {
        let ws = self.get_current_mut();
        ws.active_window = Some(window.clone());
        ws.space.map_element(window, (0, 0), true);
    }

    pub fn change_focus(&mut self, direction: &Direction, loc: &mut Point<f64, Logical>) {
        let ws = self.get_current();
        let focused = self.get_active_window();
        if let Some((window, _)) = best_window(direction, &ws.space, focused) {
            *loc = window_center(&ws.space, &window).unwrap();
        }
    }

    pub fn move_window(&mut self, direction: &Direction, loc: &mut Point<f64, Logical>) {
        let ws = self.get_current();
        let Some(focused) = self.get_active_window() else {
            return;
        };

        // Find the best window to swap with
        let Some((best, _)) = best_window(direction, &ws.space, Some(focused.clone())) else {
            return;
        };

        // Avoid pointless work if same window
        if best == focused {
            return;
        }

        let focused_pos = match ws.space.element_location(&focused) {
            Some(pos) => pos,
            None => return,
        };
        let best_pos = match ws.space.element_location(&best) {
            Some(w) => w,
            None => return,
        };
        *loc = window_center(&ws.space, &best).unwrap();
        let ws = self.get_current_mut();
        ws.space.map_element(focused, best_pos, false);
        ws.space.map_element(best, focused_pos, false);
    }

    fn render_elements(&self) {}
}

pub fn is_fullscreen<'a, I>(elements: I) -> Option<&'a Window>
where
    I: Iterator<Item = &'a Window>,
{
    for element in elements {
        match element
            .user_data()
            .get::<RefCell<WindowUserData>>()?
            .borrow_mut()
            .mode
        {
            WindowMode::Fullscreen(_) => return Some(element),
            _ => {}
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

pub fn place_on_center(space: &mut Space<Window>, window: &Window) {
    let output = match space.outputs().next().cloned() {
        Some(o) => o,
        None => return,
    };

    let output_geo = match space.output_geometry(&output) {
        Some(g) => g,
        None => return,
    };

    let layer_map = layer_map_for_output(&output);
    let zone = layer_map.non_exclusive_zone();
    let area = Rectangle::new(output_geo.loc + zone.loc, zone.size);

    // Set bounds so the client knows the maximum size
    if let Some(toplevel) = window.toplevel() {
        toplevel.with_pending_state(|state| {
            state.bounds = Some(area.size);
        });
    }

    let window_geo = window.geometry();
    let x = area.loc.x + (area.size.w - window_geo.size.w) / 2;
    let y = area.loc.y + (area.size.h - window_geo.size.h) / 2;

    let location = Point::from((x, y));

    space.map_element(window.clone(), location, false);
}
