use smithay::desktop::{layer_map_for_output, Space, Window};

use crate::action::Direction;

pub struct Workspace {
    pub space: Space<Window>,
    pub active_window: Option<usize>,
    pub prev_window: Option<usize>,
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

    pub fn set_active_window(&mut self, index: Option<usize>) {
        self.get_current_mut().active_window = index
    }

    pub fn get_active_window(&self) -> Option<usize> {
        self.get_current().active_window
    }

    pub fn get_current_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_workspace]
    }

    pub fn get_current(&self) -> &Workspace {
        &self.workspaces[self.active_workspace]
    }

    pub fn active(&self) -> usize {
        return self.active_workspace;
    }

    pub fn is_workspace_empty(&self, workspace: usize) -> bool {
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
        let ws = self.get_current_mut();
        let active = match ws.active_window {
            Some(index) => index,
            None => return,
        };
        let space = &ws.space;
        let window = match space.elements().nth(active) {
            Some(window) => window.clone(),
            None => return,
        };

        ws.space.unmap_elem(&window);
        self.set_active_workspace(ws_index);
        self.insert_window(window);
    }

    pub fn remove_active_window(&mut self, window: Window) {
        self.get_current_mut().space.unmap_elem(&window);
        let len = self.get_current().space.elements().len();
        self.set_active_window(len.checked_sub(2));
        self.layout();
    }

    pub fn insert_window(&mut self, window: Window) {
        self.get_current_mut()
            .space
            .map_element(window, (0, 0), false);
        let len = self.get_current().space.elements().len();
        self.set_active_window(len.checked_sub(1));
        self.layout();
    }

    pub fn change_focus(&mut self, direction: &Direction) {
        let elements = self
            .get_current()
            .space
            .elements()
            .collect::<Vec<&Window>>();
        if let Some(current) = self.get_active_window() {
            match direction {
                Direction::Left => {
                    if current > 0 {
                        self.set_active_window(Some(0));
                    }
                }
                Direction::Right => {
                    if current == 0 && elements.len() > 0 {
                        self.set_active_window(Some(1));
                    }
                }
                Direction::Top => {
                    if current > 1 {
                        self.set_active_window(Some(current - 1));
                    }
                }
                Direction::Down => {
                    if current < elements.len() - 1 && current != 0 {
                        self.set_active_window(Some(current + 1));
                    }
                }
            }
        }
    }
    pub fn move_window(&mut self, direction: &Direction) {
        let mut elements: Vec<_> = self
            .get_current_mut()
            .space
            .elements()
            .map(|e| e.clone())
            .collect();
        if let Some(current) = self.get_active_window() {
            match direction {
                Direction::Left => {
                    // Move active window to index 0
                    if current > 0 {
                        elements.swap(0, current);
                        self.set_active_window(Some(0));
                    }
                }

                Direction::Right => {
                    // Move from 0 → 1
                    if current == 0 && elements.len() > 0 {
                        elements.swap(0, 1);
                        self.set_active_window(Some(1));
                    }
                }

                Direction::Top => {
                    if current > 1 {
                        let new_index = current - 1;
                        elements.swap(current, new_index);
                        self.set_active_window(Some(new_index));
                    }
                }

                Direction::Down => {
                    if current < elements.len() - 1 && current != 0 {
                        let new_index = current + 1;
                        elements.swap(current, new_index);
                        self.set_active_window(Some(new_index));
                    }
                }
            }
            let space = &mut self.get_current_mut().space;
            for elem in &elements {
                space.map_element(elem.clone(), (0, 0), false);
            }

            self.layout();
        }
    }

    pub fn layout(&mut self) {
        let space = &mut self.get_current_mut().space;
        space.refresh();

        // Get output
        let output = match space.outputs().next() {
            Some(o) => o.clone(),
            None => return, // no output, nothing to do
        };
        let geo = layer_map_for_output(&output).non_exclusive_zone();

        let output_width = geo.size.w;
        let output_height = geo.size.h;

        // Collect element references WITHOUT cloning the windows
        // This is the minimal allocation approach.
        let elements: Vec<_> = space.elements().map(|e| e.clone()).collect();
        let count = elements.len() as i32;

        if count == 0 {
            return;
        }

        // Precompute shared values:
        let half_width = output_width / 2;
        let vertical_height = if count > 1 {
            output_height / (count - 1)
        } else {
            output_height
        };

        // Layout pass
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

            // Configure surface pending state
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    state.size = Some((width, height).into());
                });
                toplevel.send_pending_configure();
            }

            // Map into space
            space.map_element(window, (x, y), false);
        }
    }
}
