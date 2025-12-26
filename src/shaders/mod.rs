use smithay::{
    backend::renderer::{
        element::Kind,
        gles::{
            element::PixelShaderElement, GlesPixelProgram, GlesRenderer, GlesTexProgram, Uniform,
            UniformName, UniformType,
        },
        glow::GlowRenderer,
    },
    utils::{Logical, Rectangle},
};
use std::borrow::BorrowMut;

const BORDER_SHADER: &str = include_str!("border.frag");

pub struct BorderShader(pub GlesPixelProgram);

impl BorderShader {
    pub fn element(
        renderer: &mut GlowRenderer,
        geo: Rectangle<i32, Logical>,
        alpha: f32,
        border_color: u32,
        border_thickness: f32,
    ) -> PixelShaderElement {
        let program = renderer
            .egl_context()
            .user_data()
            .get::<BorderShader>()
            .unwrap()
            .0
            .clone();

        let point = geo.size.to_point();

        let red = border_color >> 16 & 255;
        let green = border_color >> 8 & 255;
        let blue = border_color & 255;

        let border_thickness = 2.0;

        PixelShaderElement::new(
            program,
            geo,
            None,
            alpha,
            vec![
                Uniform::new("u_resolution", (point.x as f32, point.y as f32)),
                Uniform::new("border_color", (red as f32, green as f32, blue as f32)),
                Uniform::new("border_thickness", border_thickness),
            ],
            Kind::Unspecified,
        )
    }
}

pub fn compile_shaders(renderer: &mut GlowRenderer) {
    // Compile GLSL file into pixel shader.
    let renderer: &mut GlesRenderer = renderer.borrow_mut();
    let border_shader = renderer
        .compile_custom_pixel_shader(
            BORDER_SHADER,
            &[
                UniformName::new("u_resolution", UniformType::_2f),
                UniformName::new("border_color", UniformType::_3f),
                UniformName::new("border_thickness", UniformType::_1f),
            ],
        )
        .unwrap();

    // Save pixel shader in EGL rendering context.
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(|| BorderShader(border_shader));
}
