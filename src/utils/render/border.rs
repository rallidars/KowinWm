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
        renderer: &mut GlesRenderer,
        geo: Rectangle<i32, Logical>,
        alpha: f32,
        border_color: &str,
        border_thickness: f32,
    ) -> PixelShaderElement {
        let program = renderer
            .egl_context()
            .user_data()
            .get::<BorderShader>()
            .unwrap()
            .0
            .clone();

        let angle = 0.0 * std::f32::consts::PI;
        let gradient_direction = [angle.cos(), angle.sin()];
        PixelShaderElement::new(
            program,
            geo,
            None,
            1.0,
            vec![
                Uniform::new("startColor", hex_to_rgb(border_color).unwrap()),
                Uniform::new("endColor", hex_to_rgb(border_color).unwrap()),
                Uniform::new("thickness", border_thickness),
                Uniform::new("halfThickness", border_thickness * 0.5),
                Uniform::new("gradientDirection", gradient_direction),
            ],
            Kind::Unspecified,
        )
    }
}

pub fn compile_shaders(renderer: &mut GlesRenderer) {
    // Compile GLSL file into pixel shader.
    let border_shader = renderer
        .compile_custom_pixel_shader(
            BORDER_SHADER,
            &[
                UniformName::new("startColor", UniformType::_3f),
                UniformName::new("endColor", UniformType::_3f),
                UniformName::new("thickness", UniformType::_1f),
                UniformName::new("halfThickness", UniformType::_1f),
                UniformName::new("gradientDirection", UniformType::_2f),
            ],
        )
        .unwrap();

    // Save pixel shader in EGL rendering context.
    renderer
        .egl_context()
        .user_data()
        .insert_if_missing(|| BorderShader(border_shader));
}

fn hex_to_rgb(hex: &str) -> Result<[f32; 3], &'static str> {
    let hex = hex.trim_start_matches('#');

    if hex.len() != 6 {
        return Err("Hex color must be 6 characters");
    }

    let value = u32::from_str_radix(hex, 16).map_err(|_| "Invalid hex string")?;

    let r = ((value >> 16) & 0xFF) as f32 / 255.0;
    let g = ((value >> 8) & 0xFF) as f32 / 255.0;
    let b = (value & 0xFF) as f32 / 255.0;

    Ok([r, g, b])
}
