use std::{collections::HashMap, io, path::PathBuf, time::Duration};

use crate::{
    render::CustomRenderElements,
    shaders::{compile_shaders, BorderShader},
    state::{Backend, CalloopData, State},
};
use smithay::{
    backend::{
        allocator::{
            dmabuf::Dmabuf,
            gbm::{self, GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            self,
            compositor::{DrmCompositor, FrameFlags, RenderFrameResult},
            exporter::gbm::GbmFramebufferExporter,
            DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmNode, NodeType,
        },
        egl::{EGLDevice, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            damage::OutputDamageTracker,
            element::{
                surface::WaylandSurfaceRenderElement,
                texture::{TextureBuffer, TextureRenderElement},
                AsRenderElements, Kind, Wrap,
            },
            gles::GlesTexture,
            glow::GlowRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer},
            ImportDma, ImportEgl,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{self, UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    delegate_dmabuf,
    desktop::{
        layer_map_for_output, space::SpaceElement, utils::OutputPresentationFeedback, LayerSurface,
        Window,
    },
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, RegistrationToken,
        },
        drm::{
            control::{crtc, ModeTypeFlags},
            Device as DrmDeviceTrait,
        },
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{backend::GlobalId, Display, DisplayHandle},
    },
    utils::{DeviceFd, Scale, Transform},
    wayland::{
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shell::wlr_layer::Layer,
    },
};
use smithay_drm_extras::{
    display_info::for_connector,
    drm_scanner::{DrmScanEvent, DrmScanner},
};

const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

pub struct UdevData {
    pub session: LibSeatSession,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGlesBackend<GlowRenderer, DrmDeviceFd>>,
    devices: HashMap<DrmNode, Device>,
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
    //damage_tracker: Option<OutputDamageTracker>,
}

impl DmabufHandler for State<UdevData> {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.backend_data.dmabuf_state.as_mut().unwrap().0
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self
            .backend_data
            .gpus
            .single_renderer(&self.backend_data.primary_gpu)
            .and_then(|mut renderer| renderer.import_dmabuf(&dmabuf, None))
            .is_ok()
        {
            let _ = notifier.successful::<State<UdevData>>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State<UdevData>);

pub struct Device {
    pub surfaces: HashMap<crtc::Handle, Surface>,
    pub gbm: GbmDevice<DrmDeviceFd>,
    pub drm: DrmDevice,
    pub drm_scanner: DrmScanner,
    pub render_node: DrmNode,
    pub registration_token: RegistrationToken,
}

pub static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../../resources/cursor.rgba");
pub struct Surface {
    _device_id: DrmNode,
    _render_node: DrmNode,
    global_id: GlobalId,
    compositor: DrmCompositor<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    output: Output,
    pointer_texture: TextureBuffer<GlesTexture>,
}

impl Backend for UdevData {
    fn seat_name(&self) -> String {
        self.session.seat().clone()
    }
}

pub fn init_udev() {
    let mut event_loop: EventLoop<CalloopData<UdevData>> = EventLoop::try_new().unwrap();
    let display: Display<State<UdevData>> = Display::new().unwrap();
    let mut display_handle: DisplayHandle = display.handle().clone();
    /*
     Initialize session
    */
    let (session, seat_notifier) = match LibSeatSession::new() {
        Ok(ret) => ret,
        Err(err) => {
            tracing::error!("Could not initialize a session: {}", err);
            return;
        }
    };

    /*
     * Intitialize compositor
     */

    let (primary_gpu, _) = primary_gpu(&session.seat());
    tracing::info!("Using {} as primary gpu.", primary_gpu);

    let gpus = GpuManager::new(Default::default()).unwrap();

    let data = UdevData {
        session,
        primary_gpu,
        gpus,
        devices: HashMap::new(),
        dmabuf_state: None,
        //damage_tracker: None,
    };

    let mut state = State::new(event_loop.handle(), event_loop.get_signal(), display, data);
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context
        .udev_assign_seat(&state.backend_data.session.seat())
        .unwrap();

    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, calloopdata| {
            calloopdata.state.process_input_event(event);
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(seat_notifier, move |event, _, data| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                tracing::info!("pausing session");
            }
            SessionEvent::ActivateSession => {
                libinput_context.resume().unwrap();
                tracing::info!("pausing session");
            }
        })
        .unwrap();
    /*
     * Initialize udev
     */

    let backend = UdevBackend::new(&state.backend_data.seat_name()).unwrap();
    for (device_id, path) in backend.device_list() {
        tracing::info!("udev device {}", path.display());
        state.on_udev_event(
            UdevEvent::Added {
                device_id,
                path: path.to_owned(),
            },
            &mut display_handle,
        );
    }

    event_loop
        .handle()
        .insert_source(backend, |event, _, calloopdata| {
            calloopdata
                .state
                .on_udev_event(event, &mut calloopdata.display_handle)
        })
        .unwrap();

    let mut renderer = state
        .backend_data
        .gpus
        .single_renderer(&primary_gpu)
        .unwrap();

    tracing::info!(
        ?primary_gpu,
        "Trying to initialize EGL Hardware Acceleration",
    );
    match renderer.bind_wl_display(&display_handle) {
        Ok(_) => tracing::info!("EGL hardware-acceleration enabled"),
        Err(err) => tracing::info!(?err, "Failed to initialize EGL hardware-acceleration"),
    }

    // init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats().into_iter().collect::<Vec<_>>();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state
        .create_global_with_default_feedback::<State<UdevData>>(&display_handle, &default_feedback);
    state.backend_data.dmabuf_state = Some((dmabuf_state, global));

    let mut calloopdata = CalloopData {
        state,
        display_handle,
    };

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &calloopdata.state.socket_name);
    }

    event_loop
        .run(None, &mut calloopdata, move |data| {
            data.state
                .workspaces
                .get_current()
                .space
                .elements()
                .for_each(|e| e.refresh());

            let output = data
                .state
                .workspaces
                .get_current()
                .space
                .outputs()
                .next()
                .unwrap();
            for layer in layer_map_for_output(output).layers() {
                layer.send_frame(
                    output,
                    data.state.start_time.elapsed(),
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            }

            data.display_handle.flush_clients().unwrap();
            data.state.popup_manager.cleanup();
        })
        .unwrap();
}

// Udev
impl State<UdevData> {
    pub fn on_udev_event(&mut self, event: UdevEvent, display: &mut DisplayHandle) {
        match event {
            UdevEvent::Added { device_id, path } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_added(node, path, display);
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_changed(node, display);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_removed(node);
                }
            }
        }
    }

    fn on_device_added(&mut self, node: DrmNode, path: PathBuf, display: &mut DisplayHandle) {
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
        let egl_display = unsafe { EGLDisplay::new(gbm.clone()).unwrap() };

        let render_node = match EGLDevice::device_for_display(&egl_display)
            .ok()
            .and_then(|x| x.try_get_render_node().ok().flatten())
        {
            Some(node) => node,
            None => node,
        };

        self.backend_data
            .gpus
            .as_mut()
            .add_node(render_node, gbm.clone())
            .unwrap();

        let registration_token = self
            .loop_handle
            .insert_source(drm_notifier, move |event, meta, calloopdata| {
                calloopdata.state.on_drm_event(node, event, meta);
            })
            .unwrap();

        self.backend_data.devices.insert(
            node,
            Device {
                drm,
                gbm,
                surfaces: Default::default(),
                drm_scanner: Default::default(),
                render_node,
                registration_token,
            },
        );

        self.on_device_changed(node, display);
    }
    fn on_device_changed(&mut self, node: DrmNode, display: &mut DisplayHandle) {
        if let Some(device) = self.backend_data.devices.get_mut(&node) {
            for event in device
                .drm_scanner
                .scan_connectors(&device.drm)
                .expect("scan")
            {
                self.on_connector_event(node, event, display);
            }
        }
    }
    fn on_device_removed(&mut self, node: DrmNode) {
        if let Some(device) = self.backend_data.devices.get_mut(&node) {
            self.backend_data
                .gpus
                .as_mut()
                .remove_node(&device.render_node);

            for surface in device.surfaces.values() {
                self.display_handle
                    .disable_global::<State<UdevData>>(surface.global_id.clone());

                for workspace in self.workspaces.workspaces.iter_mut() {
                    workspace.space.unmap_output(&surface.output)
                }
            }
        }
    }
}

// Drm
impl State<UdevData> {
    pub fn on_drm_event(
        &mut self,
        node: DrmNode,
        event: drm::DrmEvent,
        _meta: &mut Option<drm::DrmEventMetadata>,
    ) {
        match event {
            drm::DrmEvent::VBlank(crtc) => {
                let device = self.backend_data.devices.get_mut(&node).unwrap();
                let surface = device.surfaces.get_mut(&crtc).unwrap();
                surface.compositor.frame_submitted().ok();
                tracing::debug!("VBlank event on {:?}", crtc);
                self.render(node, crtc).unwrap();
            }
            drm::DrmEvent::Error(_) => {}
        }
    }

    pub fn on_connector_event(
        &mut self,
        node: DrmNode,
        event: DrmScanEvent,
        display: &mut DisplayHandle,
    ) {
        let device = if let Some(device) = self.backend_data.devices.get_mut(&node) {
            device
        } else {
            tracing::error!("Received connector event for unknown device: {:?}", node);
            return;
        };

        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => {
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

                let drm_mode = *connector
                    .modes()
                    .iter()
                    .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
                    .unwrap_or(&connector.modes()[0]);

                let drm_surface = device
                    .drm
                    .create_surface(crtc, drm_mode, &[connector.handle()])
                    .unwrap();

                let (make, model) = for_connector(&device.drm, connector.handle())
                    .map(|info| (info.make().unwrap(), info.model().unwrap()))
                    .unwrap_or_else(|| ("Unknown".into(), "Unknown".into()));

                let (w, h) = connector.size().unwrap_or((0, 0));
                let output = Output::new(
                    name,
                    PhysicalProperties {
                        size: (w as i32, h as i32).into(),
                        subpixel: smithay::output::Subpixel::Unknown,
                        make,
                        model,
                    },
                );
                let global = output.create_global::<State<UdevData>>(display);
                let output_mode = WlMode::from(drm_mode);
                output.set_preferred(output_mode);
                output.change_current_state(
                    Some(output_mode),
                    Some(Transform::Normal),
                    Some(smithay::output::Scale::Integer(1)),
                    None,
                );
                let render_formats = renderer
                    .as_mut()
                    .egl_context()
                    .dmabuf_render_formats()
                    .clone();
                let gbm_allocator = GbmAllocator::new(
                    device.gbm.clone(),
                    GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
                );

                let driver = match device.drm.get_driver() {
                    Ok(driver) => driver,
                    Err(err) => {
                        tracing::warn!("Failed to query drm driver: {}", err);
                        return;
                    }
                };

                let mut planes = drm_surface.planes().clone();

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
                let framebuffer_exporter =
                    GbmFramebufferExporter::new(device.gbm.clone(), Some(node).into());

                let compositor = DrmCompositor::new(
                    &output,
                    drm_surface,
                    Some(planes),
                    gbm_allocator,
                    framebuffer_exporter,
                    SUPPORTED_FORMATS.to_vec(),
                    render_formats,
                    device.drm.cursor_size(),
                    Some(device.gbm.clone()),
                )
                .unwrap();

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
                    compositor,
                    pointer_texture,
                    output: output.clone(),
                    global_id: global,
                };
                for workspace in self.workspaces.workspaces.iter_mut() {
                    workspace.space.map_output(&output, (0, 0));
                }

                device.surfaces.insert(crtc, surface);

                self.render(node, crtc).ok();
            }
            DrmScanEvent::Disconnected {
                crtc: Some(crtc), ..
            } => {
                device.surfaces.remove(&crtc);
            }
            _ => {}
        }
    }
}

pub fn primary_gpu(seat: &str) -> (DrmNode, PathBuf) {
    // TODO: can't this be in smithay?
    // primary_gpu() does the same thing anyway just without `NodeType::Render` check
    // so perhaps `primary_gpu(seat, node_type)`?
    udev::primary_gpu(seat)
        .unwrap()
        .and_then(|p| {
            DrmNode::from_path(&p)
                .ok()?
                .node_with_type(NodeType::Render)?
                .ok()
                .map(|node| (node, p))
        })
        .unwrap_or_else(|| {
            udev::all_gpus(seat)
                .unwrap()
                .into_iter()
                .find_map(|p| {
                    DrmNode::from_path(&p)
                        .ok()?
                        .node_with_type(NodeType::Render)?
                        .ok()
                        .map(|node| (node, p))
                })
                .expect("No GPU!")
        })
}

impl State<UdevData> {
    pub fn render(&mut self, node: DrmNode, crtc: crtc::Handle) -> Result<bool, SwapBuffersError> {
        let device = self.backend_data.devices.get_mut(&node).unwrap();
        let surface = device.surfaces.get_mut(&crtc).unwrap();
        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&device.render_node)
            .unwrap();
        let current_space = &self.workspaces.get_current().space;
        let output = current_space.outputs().next().unwrap();

        let mut renderelements: Vec<CustomRenderElements<_>> = vec![];
        let scale = Scale::from(1.0);
        renderelements.append(&mut vec![CustomRenderElements::from(
            TextureRenderElement::from_texture_buffer(
                self.pointer_location.to_physical(scale),
                &surface.pointer_texture,
                None,
                None,
                None,
                Kind::Cursor,
            ),
        )]);

        let layer_map = layer_map_for_output(output);
        let (lower, upper): (Vec<&LayerSurface>, Vec<&LayerSurface>) = layer_map
            .layers()
            .rev()
            .partition(|s| matches!(s.layer(), Layer::Background | Layer::Bottom));

        renderelements.extend(
            upper
                .into_iter()
                .filter_map(|surface| {
                    layer_map
                        .layer_geometry(surface)
                        .map(|geo| (geo.loc, surface))
                })
                .flat_map(|(loc, surface)| {
                    AsRenderElements::<MultiRenderer<_, _>>::render_elements::<
                        WaylandSurfaceRenderElement<MultiRenderer<_, _>>,
                    >(
                        surface,
                        &mut renderer,
                        loc.to_physical_precise_round(1),
                        Scale::from(1.0),
                        1.0,
                    )
                    .into_iter()
                    .map(CustomRenderElements::Window)
                }),
        );

        let active_window = &self.workspaces.get_current().active_window;
        let thickness = self.config.border.thickness;
        for window in current_space.elements() {
            let geo = current_space.element_geometry(window).unwrap();

            //geo.loc += (self.config.border.thickness, self.config.border.thickness).into();

            let color = if Some(window) == active_window.as_ref() {
                self.config.border.active
            } else {
                self.config.border.inactive
            };

            let border =
                BorderShader::element(renderer.as_mut(), geo, 1.0, color, thickness as f32);

            renderelements.push(CustomRenderElements::Shader(border));
            let location = geo.loc - window.geometry().loc;

            renderelements.extend(
                window
                    .render_elements(&mut renderer, location.to_physical(1), scale, 1.0)
                    .into_iter()
                    .map(CustomRenderElements::Window),
            );
        }

        renderelements.extend(
            lower
                .into_iter()
                .filter_map(|surface| {
                    layer_map
                        .layer_geometry(surface)
                        .map(|geo| (geo.loc, surface))
                })
                .flat_map(|(loc, surface)| {
                    AsRenderElements::<MultiRenderer<_, _>>::render_elements::<
                        WaylandSurfaceRenderElement<MultiRenderer<_, _>>,
                    >(
                        surface,
                        &mut renderer,
                        loc.to_physical_precise_round(1),
                        Scale::from(1.0),
                        1.0,
                    )
                    .into_iter()
                    .map(CustomRenderElements::Window)
                }),
        );

        let frame_result: Result<RenderFrameResult<_, _, _>, SwapBuffersError> = surface
            .compositor
            .render_frame::<_, _>(
                &mut renderer,
                &renderelements,
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
                    .compositor
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
                    data.state.render(node, crtc).ok();
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }

        self.workspaces
            .get_current()
            .space
            .elements()
            .for_each(|window| {
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
