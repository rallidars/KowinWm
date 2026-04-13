use crate::{
    state::State,
    udev::surface::Surface,
    utils::render::{border::compile_shaders, CustomRenderElements, GlMultiRenderer},
    FALLBACK_CURSOR_DATA,
};
use smithay::{
    backend::{
        allocator::{
            format::FormatSet,
            gbm::{self, GbmAllocator, GbmBufferFlags},
            Fourcc,
        },
        drm::{
            self,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutputManager, DrmOutputRenderElements},
            DrmDeviceFd, DrmNode,
        },
        egl::{EGLDevice, EGLDisplay},
        renderer::element::texture::TextureBuffer,
        session::Session,
    },
    desktop::utils::OutputPresentationFeedback,
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::RegistrationToken,
        drm::{
            control::{
                connector::Info,
                crtc::{self},
                ModeTypeFlags,
            },
            Device as DrmDeviceTrait,
        },
        gbm::Modifier,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Logical, Point, Transform},
};
use smithay_drm_extras::{
    display_info::{self},
    drm_scanner::DrmScanner,
};
use std::{collections::HashMap, fmt::LowerExp, path::PathBuf};

const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

pub struct Device {
    pub surfaces: HashMap<crtc::Handle, Surface>,
    pub drm_scanner: DrmScanner,
    pub render_node: DrmNode,
    pub drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    pub registration_token: RegistrationToken,
}

impl State {
    pub fn on_device_added(&mut self, node: DrmNode, path: PathBuf) {
        let fd = self
            .backend_data
            .session
            .open(
                &path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .unwrap();

        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, drm_notifier) = drm::DrmDevice::new(fd, false).unwrap();

        let gbm = gbm::GbmDevice::new(drm.device_fd().clone()).unwrap();

        // Make sure display is dropped before we call add_node

        let render_node = {
            let egl_display = unsafe { EGLDisplay::new(gbm.clone()).unwrap() };

            match EGLDevice::device_for_display(&egl_display)
                .ok()
                .and_then(|x| x.try_get_render_node().ok().flatten())
            {
                Some(node) => node,
                None => node,
            }
        };

        self.backend_data
            .gpus
            .as_mut()
            .add_node(render_node, gbm.clone())
            .unwrap();

        let registration_token = self
            .loop_handle
            .insert_source(drm_notifier, move |event, meta, calloopdata| {
                calloopdata.on_drm_event(node, event, meta);
            })
            .unwrap();

        let allocator = Some(render_node)
            .is_some()
            .then(|| {
                GbmAllocator::new(
                    gbm.clone(),
                    GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
                )
            })
            .or_else(|| {
                self.backend_data
                    .devices
                    .get(&self.backend_data.primary_gpu)
                    .or_else(|| {
                        self.backend_data.devices.values().find(|backend| {
                            Some(backend.render_node) == Some(self.backend_data.primary_gpu)
                        })
                    })
                    .map(|backend| backend.drm_output_manager.allocator().clone())
            })
            .unwrap();

        let framebuffer_exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());

        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&Some(render_node).unwrap_or(self.backend_data.primary_gpu))
            .unwrap();
        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|format| Some(render_node).is_some() || format.modifier == Modifier::Linear)
            .copied()
            .collect::<FormatSet>();

        let drm_output_manager = DrmOutputManager::new(
            drm,
            allocator,
            framebuffer_exporter,
            Some(gbm),
            SUPPORTED_FORMATS.iter().copied(),
            render_formats,
        );

        self.backend_data.devices.insert(
            node,
            Device {
                surfaces: Default::default(),
                drm_scanner: Default::default(),
                render_node,
                drm_output_manager,
                registration_token,
            },
        );

        self.on_device_changed(node);
    }
    pub fn on_device_changed(&mut self, node: DrmNode) {
        if let Some(device) = self.backend_data.devices.get_mut(&node) {
            for event in device
                .drm_scanner
                .scan_connectors(device.drm_output_manager.device())
                .expect("scan")
            {
                self.on_connector_event(node, event);
            }
        }
    }
    pub fn on_device_removed(&mut self, node: DrmNode) {
        if let Some(device) = self.backend_data.devices.get_mut(&node) {
            self.backend_data
                .gpus
                .as_mut()
                .remove_node(&device.render_node);

            for surface in device.surfaces.values() {
                self.display_handle
                    .disable_global::<State>(surface.global_id.clone());

                for workspace in self.workspaces.workspaces.iter_mut() {
                    workspace.space.unmap_output(&surface.output)
                }
            }
        }
    }
    pub fn connected(&mut self, connector: Info, crtc: crtc::Handle, node: DrmNode) {
        let device = if let Some(device) = self.backend_data.devices.get_mut(&node) {
            device
        } else {
            tracing::error!("Received connector event for unknown device: {:?}", node);
            return;
        };
        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&device.render_node)
            .unwrap();
        let name = format!(
            "{}-{}",
            connector.interface().as_str(),
            connector.interface_id()
        );
        tracing::info!("New output connected, name: {}", name);
        // Check if this output is in the config
        let config_output = self.config.outputs.get(&name);
        if let Some(output_data) = config_output {
            if !output_data.enabled {
                tracing::info!("Output {} disabled in config, skipping", name);
                return;
            }
        }

        let drm_mode = *connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(&connector.modes()[0]);

        let display_info =
            display_info::for_connector(device.drm_output_manager.device(), connector.handle());

        let make = display_info
            .as_ref()
            .and_then(|info| info.make())
            .unwrap_or_else(|| "Unknown".into());

        let model = display_info
            .as_ref()
            .and_then(|info| info.model())
            .unwrap_or_else(|| "Unknown".into());

        let (w, h) = connector.size().unwrap_or((0, 0));
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: (w as i32, h as i32).into(),
                subpixel: connector.subpixel().into(),
                make,
                model,
            },
        );
        let global = output.create_global::<State>(&self.display_handle);

        let mut output_mode = WlMode::from(drm_mode);
        let (transform, scale, position) = if let Some(config) = &config_output {
            // Override refresh rate if provided
            if let Some(refresh) = config.refresh_rate {
                output_mode.refresh = refresh * 1000;
            }
            // Override resolution if provided
            if let Some(res) = config.resolution {
                output_mode.size = res.into();
            }
            // Parse transform, scale, and position (with typo preserved)
            let transform = config.transform.clone().and_then(parse_transform);
            let scale = config.scale.map(|s| smithay::output::Scale::Fractional(s));
            let position: Option<Point<i32, Logical>> = config.possition.map(Into::into); // note: field name typo
            (transform, scale, position)
        } else {
            (None, None, None)
        };

        output.set_preferred(output_mode);
        output.change_current_state(Some(output_mode), transform, scale, position);
        tracing::info!("{:?}", output.current_mode());

        for (index, ws) in self.workspaces.workspaces.iter_mut().enumerate() {
            ws.space
                .map_output(&output, position.unwrap_or((0, 0).into()));
        }

        let driver = match device.drm_output_manager.device().get_driver() {
            Ok(driver) => driver,
            Err(err) => {
                tracing::warn!("Failed to query drm driver: {}", err);
                return;
            }
        };

        let mut planes = device.drm_output_manager.device().planes(&crtc).unwrap();

        // Using an overlay plane on a nvidia card breaks
        if driver
            .name()
            .to_string_lossy()
            .to_lowercase()
            .contains("nvidia")
            || driver
                .description()
                .to_string_lossy()
                .to_lowercase()
                .contains("nvidia")
        {
            planes.overlay = vec![];
        }

        let drm_output = match device
            .drm_output_manager
            .initialize_output::<_, CustomRenderElements<GlMultiRenderer<'_>>>(
                crtc,
                drm_mode,
                &[connector.handle()],
                &output,
                Some(planes),
                &mut renderer,
                &DrmOutputRenderElements::default(),
            ) {
            Ok(drm_output) => drm_output,
            Err(err) => {
                tracing::warn!("Failed to initialize drm output: {}", err);
                return;
            }
        };

        let pointer_texture = TextureBuffer::from_memory(
            renderer.as_mut(),
            FALLBACK_CURSOR_DATA,
            Fourcc::Abgr8888,
            (64, 64),
            false,
            2,
            Transform::Normal,
            None,
        )
        .unwrap();

        // compile border shaders
        compile_shaders(renderer.as_mut());

        let surface = Surface {
            _device_id: node,
            _render_node: device.render_node,
            drm_output,
            pointer_texture,
            output: output.clone(),
            global_id: global,
        };

        device.surfaces.insert(crtc, surface);

        self.render(node, crtc).ok();
    }
}

fn parse_transform(s: String) -> Option<Transform> {
    match s.to_lowercase().as_str() {
        "normal" => Some(Transform::Normal),
        "90" | "90°" | "rotated90" => Some(Transform::_90),
        "180" | "180°" | "rotated180" => Some(Transform::_180),
        "270" | "270°" | "rotated270" => Some(Transform::_270),
        "flipped" => Some(Transform::Flipped),
        "flipped-90" | "flipped90" => Some(Transform::Flipped90),
        "flipped-180" | "flipped180" => Some(Transform::Flipped180),
        "flipped-270" | "flipped270" => Some(Transform::Flipped270),
        _ => None,
    }
}
