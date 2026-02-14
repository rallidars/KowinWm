use smithay::{
    desktop::{space::SpaceElement, Window},
    utils::{Logical, Rectangle},
};

pub enum LayoutState {
    MasterStack(MasterStack),
}

impl Default for LayoutState {
    fn default() -> Self {
        LayoutState::MasterStack(MasterStack::default())
    }
}

pub struct Placement<'a> {
    pub window: &'a Window,
    pub geometry: Rectangle<i32, Logical>,
}

pub trait LayoutBehavior {
    fn placement<'a, I>(&mut self, windows: I, area: Rectangle<i32, Logical>) -> Vec<Placement<'a>>
    where
        I: Iterator<Item = &'a Window> + ExactSizeIterator;
}

impl LayoutBehavior for LayoutState {
    fn placement<'a, I>(&mut self, windows: I, area: Rectangle<i32, Logical>) -> Vec<Placement<'a>>
    where
        I: Iterator<Item = &'a Window> + ExactSizeIterator,
    {
        match self {
            LayoutState::MasterStack(layout) => layout.placement(windows, area),
        }
    }
}

pub struct MasterStack {
    master_size: f32,
    windows: Vec<Window>,
}
impl Default for MasterStack {
    fn default() -> Self {
        Self {
            master_size: 0.5,
            windows: vec![],
        }
    }
}
impl LayoutBehavior for MasterStack {
    fn placement<'a, I>(&mut self, windows: I, area: Rectangle<i32, Logical>) -> Vec<Placement<'a>>
    where
        I: Iterator<Item = &'a Window> + ExactSizeIterator,
    {
        let mut result = Vec::new();
        let count = windows.len() as i32;

        let half_width = area.size.w / 2;
        let stack_height = if count > 1 {
            area.size.h / (count - 1)
        } else {
            area.size.h
        };

        for (i, window) in windows.enumerate() {
            let mut x = area.loc.x;
            let mut y = area.loc.y;
            let mut width = area.size.w;
            let mut height = area.size.h;

            if count > 1 {
                width = half_width;
            }

            // stack windows
            if i > 0 {
                x = area.loc.x + half_width;
                y = area.loc.y + stack_height * (i as i32 - 1);
                width = half_width;
                height = stack_height;
            }

            let geometry = Rectangle::new((x, y).into(), (width, height).into());

            result.push(Placement { window, geometry });
        }

        result
    }
}
