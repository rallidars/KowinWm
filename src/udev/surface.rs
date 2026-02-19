use std::{io, time::Duration};

use crate::{
    state::State,
    utils::{
        render::{border::BorderShader, CustomRenderElements},
        workspaces::is_fullscreen,
    },
};
use smithay::{
    backend::{
        allocator::gbm::GbmAllocator,
        drm::{
            compositor::{FrameFlags, RenderFrameResult},
            exporter::gbm::GbmFramebufferExporter,
            output::DrmOutput,
            DrmAccessError, DrmDeviceFd, DrmError, DrmNode,
        },
        renderer::{
            element::{
                surface::WaylandSurfaceRenderElement,
                texture::{TextureBuffer, TextureRenderElement},
                AsRenderElements, Kind,
            },
            gles::GlesTexture,
            multigpu::MultiRenderer,
        },
        SwapBuffersError,
    },
    desktop::{layer_map_for_output, utils::OutputPresentationFeedback},
    output::Output,
    reexports::{
        calloop::timer::{TimeoutAction, Timer},
        drm::control::crtc,
        wayland_server::backend::GlobalId,
    },
    utils::Scale,
    wayland::shell::wlr_layer::Layer,
};

pub static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../../resources/cursor.rgba");
pub struct Surface {
    pub _device_id: DrmNode,
    pub _render_node: DrmNode,
    pub global_id: GlobalId,
    pub drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    pub output: Output,
    pub pointer_texture: TextureBuffer<GlesTexture>,
}

impl State {
    pub fn render(&mut self, node: DrmNode, crtc: crtc::Handle) -> Result<bool, SwapBuffersError> {
        let device = self.backend_data.devices.get_mut(&node).unwrap();
        let surface = device.surfaces.get_mut(&crtc).unwrap();

        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&device.render_node)
            .unwrap();

        let ws = self.workspaces.get_current();
        let output = ws.space.outputs().next().unwrap();

        let scale = Scale::from(1.0);
        let physical_scale = 1;

        // ------------------------------------------------------------
        // Render element collection (NO allocations inside loops)
        // ------------------------------------------------------------
        let mut elements: Vec<CustomRenderElements<_>> = Vec::with_capacity(128);

        // ------------------------------------------------------------
        // Cursor
        // ------------------------------------------------------------
        elements.push(CustomRenderElements::from(
            TextureRenderElement::from_texture_buffer(
                self.pointer_location.to_physical(scale),
                &surface.pointer_texture,
                None,
                None,
                None,
                Kind::Cursor,
            ),
        ));

        // ------------------------------------------------------------
        // Layer surfaces (TOP → BOTTOM, no Vec partition)
        // ------------------------------------------------------------
        let layer_map = layer_map_for_output(output);

        for layer_surface in layer_map.layers().rev() {
            if matches!(layer_surface.layer(), Layer::Background | Layer::Bottom) {
                continue;
            }

            if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                for elem in AsRenderElements::<MultiRenderer<_, _>>::render_elements::<
                    WaylandSurfaceRenderElement<_>,
                >(
                    layer_surface,
                    &mut renderer,
                    geo.loc.to_physical_precise_round(physical_scale),
                    scale,
                    1.0,
                ) {
                    elements.push(CustomRenderElements::Window(elem));
                }
            }
        }

        // ------------------------------------------------------------
        // Windows
        // ------------------------------------------------------------
        let border = &self.config.border;
        let active = ws.active_window.as_ref();
        let fullscreen = is_fullscreen(ws.space.elements());

        if let Some(win) = fullscreen {
            let loc = ws.space.element_location(win).unwrap();
            for elem in
                win.render_elements(&mut renderer, loc.to_physical(physical_scale), scale, 1.0)
            {
                elements.push(CustomRenderElements::Window(elem));
            }
        } else {
            for window in ws.space.elements().rev() {
                // Geometry cached once
                let geo = ws.space.element_geometry(&window).unwrap();
                let loc = ws.space.element_location(&window).unwrap();
                let win_geo = window.geometry();

                // Border
                let mut border_geo = geo;
                border_geo.size += (border.thickness * 2, border.thickness * 2).into();
                border_geo.loc -= (border.thickness, border.thickness).into();

                let color = if Some(window) == active {
                    border.active.clone()
                } else {
                    border.inactive.clone()
                };

                let border_elem = BorderShader::element(
                    renderer.as_mut(),
                    border_geo,
                    1.0,
                    &color,
                    border.thickness as f32,
                );

                elements.push(CustomRenderElements::Shader(border_elem));

                // Window content
                let offset = loc - win_geo.loc;
                for elem in window.render_elements(
                    &mut renderer,
                    offset.to_physical(physical_scale),
                    scale,
                    1.0,
                ) {
                    elements.push(CustomRenderElements::Window(elem));
                }
            }
        }

        // ------------------------------------------------------------
        // Bottom layers
        // ------------------------------------------------------------
        for layer_surface in layer_map.layers().rev() {
            if !matches!(layer_surface.layer(), Layer::Background | Layer::Bottom) {
                continue;
            }

            if let Some(geo) = layer_map.layer_geometry(layer_surface) {
                for elem in AsRenderElements::<MultiRenderer<_, _>>::render_elements::<
                    WaylandSurfaceRenderElement<_>,
                >(
                    layer_surface,
                    &mut renderer,
                    geo.loc.to_physical_precise_round(physical_scale),
                    scale,
                    1.0,
                ) {
                    elements.push(CustomRenderElements::Window(elem));
                }
            }
        }

        let frame_result: Result<RenderFrameResult<_, _, _>, SwapBuffersError> = surface
            .drm_output
            .render_frame::<_, _>(
                &mut renderer,
                &elements,
                [0.1, 0.1, 0.1, 1.0],
                FrameFlags::DEFAULT,
            )
            .map_err(|err| match err {
                smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => {
                    err.into()
                }
                smithay::backend::drm::compositor::RenderFrameError::RenderFrame(
                    smithay::backend::renderer::damage::Error::Rendering(err),
                ) => err.into(),
                _ => unreachable!(),
            });

        let mut result = match frame_result {
            Ok(frame_result) => Ok(!frame_result.is_empty),
            Err(frame_result) => Err(frame_result),
        };

        if let Ok(rendered) = result {
            if rendered {
                let queueresult = surface
                    .drm_output
                    .queue_frame(None)
                    .map_err(Into::<SwapBuffersError>::into);
                if let Err(queueresult) = queueresult {
                    result = Err(queueresult);
                }
            }
        }

        let reschedule = match &result {
            Ok(has_rendered) => !has_rendered,
            Err(err) => {
                tracing::warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err)
                        if matches!(
                            err.downcast_ref::<DrmError>(),
                            Some(&DrmError::DeviceInactive)
                        ) =>
                    {
                        false
                    }
                    SwapBuffersError::TemporaryFailure(err) => matches!(
                        err.downcast_ref::<DrmError>(),
                        Some(DrmError::Access(DrmAccessError {source, ..})) if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => {
                        tracing::warn!("Rendering loop lost: {}", err);
                        false
                    }
                }
            }
        };

        if reschedule {
            let output_refresh = match output.current_mode() {
                Some(mode) => mode.refresh,
                None => return result,
            };
            // If reschedule is true we either hit a temporary failure or more likely rendering
            // did not cause any damage on the output. In this case we just re-schedule a repaint
            // after approx. one frame to re-test for damage.
            let reschedule_duration =
                Duration::from_millis((1_000_000f32 / output_refresh as f32) as u64);
            tracing::trace!(
                "reschedule repaint timer with delay {:?} on {:?}",
                reschedule_duration,
                crtc,
            );
            let timer = Timer::from_duration(reschedule_duration);
            self.loop_handle
                .insert_source(timer, move |_, _, data| {
                    data.render(node, crtc).ok();
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }

        ws.space.elements().for_each(|window| {
            window.send_frame(
                output,
                self.start_time.elapsed(),
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        });
        result
    }
}
