mod device;
mod surface;

use std::{
    cell::RefCell,
    collections::HashMap,
    io,
    ops::Not,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Once},
    time::{Duration, Instant},
};

use crate::{
    state::State,
    utils::{
        config::Config,
        render::{
            border::{compile_shaders, BorderShader},
            CustomRenderElements, GlMultiRenderer,
        },
        workspaces::{is_fullscreen, WindowMode, WindowUserData, Workspace},
    },
};
use smithay::{
    backend::{
        allocator::{
            dmabuf::Dmabuf,
            format::FormatSet,
            gbm::{self, GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            self,
            compositor::{DrmCompositor, FrameFlags, RenderFrameResult},
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            CreateDrmNodeError, DrmAccessError, DrmDevice, DrmDeviceFd, DrmError, DrmEvent,
            DrmEventMetadata, DrmEventTime, DrmNode, DrmSurface, NodeType,
        },
        egl::{self, EGLDevice, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            element::{
                default_primary_scanout_output_compare,
                surface::WaylandSurfaceRenderElement,
                texture::{TextureBuffer, TextureRenderElement},
                utils::select_dmabuf_feedback,
                AsRenderElements, Kind, RenderElementStates, Wrap,
            },
            gles::{GlesRenderer, GlesTexture},
            glow::GlowRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager, MultiRenderer},
            Color32F, ImportDma, ImportEgl, ImportMemWl,
        },
        session::{
            libseat::{self, LibSeatSession},
            Event as SessionEvent, Session,
        },
        udev::{self, UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    delegate_dmabuf, delegate_drm_lease,
    desktop::{
        layer_map_for_output,
        space::SpaceElement,
        utils::{
            surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
            update_surface_primary_scanout_output, OutputPresentationFeedback,
        },
        LayerSurface, Space, Window,
    },
    output::{Mode as WlMode, Output, PhysicalProperties},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, RegistrationToken,
        },
        drm::{
            control::{connector, crtc, ModeTypeFlags},
            Device as DrmDeviceTrait,
        },
        gbm::Modifier,
        input::Libinput,
        rustix::fs::OFlags,
        wayland_protocols::wp::{
            linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1,
            presentation_time::server::wp_presentation_feedback,
        },
        wayland_server::{
            backend::{ClientId, GlobalId},
            protocol::wl_surface,
            Client, Display, DisplayHandle, Resource,
        },
    },
    utils::{DeviceFd, Logical, Monotonic, Point, Scale, Time, Transform},
    wayland::{
        commit_timing::CommitTimerBarrierStateUserData,
        compositor::CompositorHandler,
        dmabuf::{
            DmabufFeedback, DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState,
            ImportNotifier,
        },
        drm_lease::{
            DrmLease, DrmLeaseBuilder, DrmLeaseHandler, DrmLeaseRequest, DrmLeaseState,
            LeaseRejected,
        },
        drm_syncobj::{supports_syncobj_eventfd, DrmSyncobjHandler, DrmSyncobjState},
        fifo::FifoBarrierCachedState,
        fractional_scale::with_fractional_scale,
        presentation::Refresh,
        shell::wlr_layer::Layer,
    },
};
use smithay_drm_extras::{
    display_info::{self, for_connector},
    drm_scanner::{DrmScanEvent, DrmScanner},
};
#[derive(Debug, PartialEq)]
struct UdevOutputId {
    device_id: DrmNode,
    crtc: crtc::Handle,
}

pub static CLEAR_COLOR: Color32F = Color32F::new(0.8, 0.8, 0.9, 1.0);
type UdevRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlesRenderer, DrmDeviceFd>,
>;

const SUPPORTED_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

pub struct UdevData {
    pub session: LibSeatSession,
    syncobj_state: Option<DrmSyncobjState>,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    devices: HashMap<DrmNode, Device>,
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
}

#[derive(Debug, thiserror::Error)]
enum DeviceAddError {
    #[error("Failed to open device using libseat: {0}")]
    DeviceOpen(libseat::Error),
    #[error("Failed to initialize drm device: {0}")]
    DrmDevice(DrmError),
    #[error("Failed to initialize gbm device: {0}")]
    GbmDevice(std::io::Error),
    #[error("Failed to access drm node: {0}")]
    DrmNode(CreateDrmNodeError),
    #[error("Failed to add device to GpuManager: {0}")]
    AddNode(egl::Error),
    #[error("The device has no render node")]
    NoRenderNode,
    #[error("Primary GPU is missing")]
    PrimaryGpuMissing,
}

impl DmabufHandler for State {
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
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State);

impl UdevData {
    pub fn early_import(&mut self, surface: &wl_surface::WlSurface) {
        if let Err(err) = self.gpus.early_import(self.primary_gpu, surface) {
            tracing::warn!("Early buffer import failed: {}", err);
        }
    }
}

pub struct Device {
    pub surfaces: HashMap<crtc::Handle, Surface>,
    pub drm_output_manager: DrmOutputManager<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    non_desktop_connectors: Vec<(connector::Handle, crtc::Handle)>,
    leasing_global: Option<DrmLeaseState>,
    active_leases: Vec<DrmLease>,
    pub drm_scanner: DrmScanner,
    pub render_node: DrmNode,
    pub registration_token: RegistrationToken,
}

pub static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../../resources/cursor.rgba");
pub struct Surface {
    device_id: DrmNode,
    render_node: DrmNode,
    global_id: GlobalId,
    output: Output,
    drm_output: DrmOutput<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        Option<OutputPresentationFeedback>,
        DrmDeviceFd,
    >,
    pointer_texture: TextureBuffer<GlesTexture>,
    dmabuf_feedback: Option<SurfaceDmabufFeedback>,
    last_presentation_time: Option<Time<Monotonic>>,
    vblank_throttle_timer: Option<RegistrationToken>,
}

pub fn init_udev() {
    let mut event_loop: EventLoop<State> = EventLoop::try_new().unwrap();
    let display: Display<State> = Display::new().unwrap();

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
        syncobj_state: None,
    };

    /*
     * Initialize libinput state
     */

    let mut state = State::new(event_loop.handle(), event_loop.get_signal(), display, data);
    /*
     * Initialize udev
     */

    let backend = UdevBackend::new(&state.backend_data.session.seat()).unwrap();

    /*
     * Initialize libinput backend
     */
    let mut libinput_context = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(
        state.backend_data.session.clone().into(),
    );
    libinput_context
        .udev_assign_seat(&state.backend_data.session.seat())
        .unwrap();

    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    /*
     * Bind all our objects that get driven by the event loop
     */
    event_loop
        .handle()
        .insert_source(libinput_backend, move |event, _, data| {
            data.process_input_event(event);
        })
        .unwrap();

    event_loop
        .handle()
        .insert_source(seat_notifier, move |event, _, data| match event {
            SessionEvent::PauseSession => {
                libinput_context.suspend();
                for backend in data.backend_data.devices.values_mut() {
                    backend.drm_output_manager.pause();
                    backend.active_leases.clear();
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.suspend();
                    }
                }
                tracing::info!("pausing session");
            }
            SessionEvent::ActivateSession => {
                tracing::info!("resuming session");

                if let Err(err) = libinput_context.resume() {
                    tracing::error!("Failed to resume libinput context: {:?}", err);
                }
                for (node, backend) in data
                    .backend_data
                    .devices
                    .iter_mut()
                    .map(|(handle, backend)| (*handle, backend))
                {
                    backend
                        .drm_output_manager
                        .activate(false)
                        .expect("failed to activate drm backend");
                    if let Some(lease_global) = backend.leasing_global.as_mut() {
                        lease_global.resume::<State>();
                    }
                    data.loop_handle
                        .insert_idle(move |data| data.render(node, None, data.clock.now()));
                }
            }
        })
        .unwrap();
    // any display only node can fall back to the primary node for rendering
    let primary_node = primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|node| node.ok());

    let primary_device = backend.device_list().find(|(device_id, _)| {
        primary_node
            .map(|primary_node| *device_id == primary_node.dev_id())
            .unwrap_or(false)
            || *device_id == primary_gpu.dev_id()
    });

    if let Some((device_id, path)) = primary_device {
        let node = DrmNode::from_dev_id(device_id).expect("failed to get primary node");
        state.on_device_added(node, path)
    }

    let primary_device_id = primary_device.map(|(device_id, _)| device_id);
    for (device_id, path) in backend.device_list() {
        if Some(device_id) == primary_device_id {
            continue;
        }

        if let Err(err) = DrmNode::from_dev_id(device_id)
            .map_err(DeviceAddError::DrmNode)
            .and_then(|node| Ok(state.on_device_added(node, path)))
        {
            tracing::error!("Skipping device {device_id}: {err}");
        }
    }
    state.shm_state.update_formats(
        state
            .backend_data
            .gpus
            .single_renderer(&primary_gpu)
            .unwrap()
            .shm_formats(),
    );

    #[cfg_attr(not(feature = "egl"), allow(unused_mut))]
    let mut renderer = state
        .backend_data
        .gpus
        .single_renderer(&primary_gpu)
        .unwrap();

    //{
    //    tracing::info!(
    //        ?primary_gpu,
    //        "Trying to initialize EGL Hardware Acceleration",
    //    );
    //    match renderer.bind_wl_display(&display_handle) {
    //        Ok(_) => tracing::info!("EGL hardware-acceleration enabled"),
    //        Err(err) => tracing::info!(?err, "Failed to initialize EGL hardware-acceleration"),
    //    }
    //}
    // init dmabuf support with format list from our primary gpu
    let dmabuf_formats = renderer.dmabuf_formats();
    let default_feedback = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), dmabuf_formats)
        .build()
        .unwrap();
    let mut dmabuf_state = DmabufState::new();
    let global = dmabuf_state
        .create_global_with_default_feedback::<State>(&state.display_handle, &default_feedback);
    state.backend_data.dmabuf_state = Some((dmabuf_state, global));

    let gpus = &mut state.backend_data.gpus;
    state
        .backend_data
        .devices
        .iter_mut()
        .for_each(|(node, backend_data)| {
            // Update the per drm surface dmabuf feedback
            backend_data.surfaces.values_mut().for_each(|surface_data| {
                surface_data.dmabuf_feedback = surface_data.dmabuf_feedback.take().or_else(|| {
                    surface_data.drm_output.with_compositor(|compositor| {
                        get_surface_dmabuf_feedback(
                            primary_gpu,
                            Some(surface_data.render_node),
                            *node,
                            gpus,
                            compositor.surface(),
                        )
                    })
                });
            });
        });

    // Expose syncobj protocol if supported by primary GPU
    if let Some(primary_node) = state
        .backend_data
        .primary_gpu
        .node_with_type(NodeType::Primary)
        .and_then(|x| x.ok())
    {
        if let Some(backend) = state.backend_data.devices.get(&primary_node) {
            let import_device = backend.drm_output_manager.device().device_fd().clone();
            if supports_syncobj_eventfd(&import_device) {
                let syncobj_state =
                    DrmSyncobjState::new::<State>(&state.display_handle, import_device);
                state.backend_data.syncobj_state = Some(syncobj_state);
            }
        }
    }
    event_loop
        .handle()
        .insert_source(backend, move |event, _, data| match event {
            UdevEvent::Added { device_id, path } => {
                if let Err(err) = DrmNode::from_dev_id(device_id)
                    .map_err(DeviceAddError::DrmNode)
                    .and_then(|node| Ok(data.on_device_added(node, &path)))
                {
                    tracing::error!("Skipping device {device_id}: {err}");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.on_device_changed(node)
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    data.on_device_removed(node)
                }
            }
        })
        .unwrap();

    /*
     * Start XWayland if supported
     */
    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    /*
     * And run our loop
     */

    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
    }

    /*
     * Start XWayland if supported
     */

    #[cfg(feature = "xwayland")]
    state.start_xwayland();

    /*
     * And run our loop
     */

    //for program in autostart {
    //    std::process::Command::new("/bin/sh")
    //        .arg("-c")
    //        .arg(&program)
    //        .spawn()
    //        .map_err(|e| tracing::info!("Failed to spawn '{program}': {e}"))
    //        .ok();
    //}

    while state.running.load(Ordering::SeqCst) {
        let result = event_loop.dispatch(Some(Duration::from_millis(16)), &mut state);
        if result.is_err() {
            state.running.store(false, Ordering::SeqCst);
        } else {
            for ws in state.workspaces.workspaces.iter() {
                ws.space.elements().for_each(|e| e.refresh());
            }

            //let output = state
            //    .workspaces
            //    .get_current()
            //    .space
            //    .outputs()
            //    .next()
            //    .unwrap();
            //for layer in layer_map_for_output(output).layers() {
            //    layer.send_frame(
            //        output,
            //        state.start_time.elapsed(),
            //        Some(Duration::ZERO),
            //        |_, _| Some(output.clone()),
            //    );
            //}

            state.popup_manager.cleanup();
            state.display_handle.flush_clients().unwrap();
        }
    }
    //event_loop
    //    .run(None, &mut state, move |data| {
    //        for ws in data.workspaces.workspaces.iter() {
    //            ws.space.elements().for_each(|e| e.refresh());
    //        }

    //        let output = data
    //            .workspaces
    //            .get_current()
    //            .space
    //            .outputs()
    //            .next()
    //            .unwrap();
    //        for layer in layer_map_for_output(output).layers() {
    //            layer.send_frame(
    //                output,
    //                data.start_time.elapsed(),
    //                Some(Duration::ZERO),
    //                |_, _| Some(output.clone()),
    //            );
    //        }

    //        data.display_handle.flush_clients().unwrap();
    //        data.popup_manager.cleanup();
    //    })
    //    .unwrap();
}

// Udev
impl State {
    fn on_device_added(&mut self, node: DrmNode, path: &Path) {
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

        let registration_token = self
            .loop_handle
            .insert_source(
                drm_notifier,
                move |event, metadata, data: &mut State| match event {
                    DrmEvent::VBlank(crtc) => {
                        tracing::info!("enter vblank: {:?}", crtc);
                        data.frame_finish(node, crtc, metadata);
                    }
                    DrmEvent::Error(error) => {
                        tracing::error!("{:?}", error);
                    }
                },
            )
            .unwrap();

        let mut try_initialize_gpu = || {
            let display = unsafe { EGLDisplay::new(gbm.clone()).map_err(DeviceAddError::AddNode)? };
            let egl_device =
                EGLDevice::device_for_display(&display).map_err(DeviceAddError::AddNode)?;

            if egl_device.is_software() {
                return Err(DeviceAddError::NoRenderNode);
            }

            let render_node = egl_device
                .try_get_render_node()
                .ok()
                .flatten()
                .unwrap_or(node);
            self.backend_data
                .gpus
                .as_mut()
                .add_node(render_node, gbm.clone())
                .map_err(DeviceAddError::AddNode)?;

            std::result::Result::<DrmNode, DeviceAddError>::Ok(render_node)
        };

        let render_node = try_initialize_gpu()
            .inspect_err(|err| {
                tracing::warn!(?err, "failed to initialize gpu");
            })
            .ok();

        let allocator = render_node
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
                            backend.render_node == Some(self.backend_data.primary_gpu).unwrap()
                        })
                    })
                    .map(|backend| backend.drm_output_manager.allocator().clone())
            })
            .ok_or(DeviceAddError::PrimaryGpuMissing)
            .unwrap();

        let framebuffer_exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());

        let color_formats = if std::env::var("ANVIL_DISABLE_10BIT").is_ok() {
            SUPPORTED_FORMATS
        } else {
            SUPPORTED_FORMATS
        };
        let mut renderer = self
            .backend_data
            .gpus
            .single_renderer(&render_node.unwrap_or(self.backend_data.primary_gpu))
            .unwrap();

        let render_formats = renderer
            .as_mut()
            .egl_context()
            .dmabuf_render_formats()
            .iter()
            .filter(|format| render_node.is_some() || format.modifier == Modifier::Linear)
            .copied()
            .collect::<FormatSet>();

        let drm_output_manager = DrmOutputManager::new(
            drm,
            allocator,
            framebuffer_exporter,
            Some(gbm),
            color_formats.iter().copied(),
            render_formats,
        );

        self.backend_data.devices.insert(
            node,
            Device {
                registration_token,
                drm_output_manager,
                drm_scanner: DrmScanner::new(),
                render_node: render_node.unwrap(),
                surfaces: HashMap::new(),
                non_desktop_connectors: Vec::new(),
                leasing_global: DrmLeaseState::new::<State>(&self.display_handle, &node)
                    .inspect_err(|err| {
                        tracing::warn!(?err, "Failed to initialize drm lease global for: {}", node);
                    })
                    .ok(),
                active_leases: Vec::new(),
            },
        );

        self.on_device_changed(node);
    }
    fn on_device_changed(&mut self, node: DrmNode) {
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
    fn on_device_removed(&mut self, node: DrmNode) {
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
}

// Drm
impl State {
    pub fn on_connector_event(&mut self, node: DrmNode, event: DrmScanEvent) {
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
                let render_node = device.render_node;
                let mut renderer = self
                    .backend_data
                    .gpus
                    .single_renderer(&render_node)
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

                let drm_device = device.drm_output_manager.device();

                let display_info = display_info::for_connector(drm_device, connector.handle());

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
                if let Some(config) = config_output {
                    output_mode.refresh = config.refresh_rate * 1000;
                    output_mode.size = config.resolution.into();
                    output.set_preferred(output_mode);
                    output.change_current_state(
                        Some(output_mode),
                        Some(Transform::Normal),
                        Some(smithay::output::Scale::Fractional(config.scale)),
                        None,
                    );
                } else {
                    output.set_preferred(output_mode);
                    output.change_current_state(Some(output_mode), None, None, None);
                }

                for ws in self.workspaces.workspaces.iter_mut() {
                    ws.space.map_output(&output, output.current_location());
                }
                output.user_data().insert_if_missing(|| UdevOutputId {
                    crtc,
                    device_id: node,
                });

                let driver = match drm_device.get_driver() {
                    Ok(driver) => driver,
                    Err(err) => {
                        tracing::warn!("Failed to query drm driver: {}", err);
                        return;
                    }
                };

                let mut planes = match drm_device.planes(&crtc) {
                    Ok(planes) => planes,
                    Err(err) => {
                        tracing::warn!("Failed to query crtc planes: {}", err);
                        return;
                    }
                };

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
                    .initialize_output::<_, CustomRenderElements<UdevRenderer<'_>>>(
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

                let dmabuf_feedback = drm_output.with_compositor(|compositor| {
                    get_surface_dmabuf_feedback(
                        self.backend_data.primary_gpu,
                        Some(device.render_node),
                        node,
                        &mut self.backend_data.gpus,
                        compositor.surface(),
                    )
                });

                let surface = Surface {
                    device_id: node,
                    render_node: device.render_node,
                    drm_output,
                    pointer_texture,
                    output: output.clone(),
                    global_id: global,
                    dmabuf_feedback,
                    vblank_throttle_timer: None,
                    last_presentation_time: None,
                };

                device.surfaces.insert(crtc, surface);

                self.loop_handle.insert_idle(move |state| {
                    state.render_surface(node, crtc, state.clock.now());
                });
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

impl State {
    fn frame_finish(
        &mut self,
        dev_id: DrmNode,
        crtc: crtc::Handle,
        metadata: &mut Option<DrmEventMetadata>,
    ) {
        let device_backend = match self.backend_data.devices.get_mut(&dev_id) {
            Some(backend) => backend,
            None => {
                tracing::error!("Trying to finish frame on non-existent backend {}", dev_id);
                return;
            }
        };

        let surface = match device_backend.surfaces.get_mut(&crtc) {
            Some(surface) => surface,
            None => {
                tracing::error!("Trying to finish frame on non-existent crtc {:?}", crtc);
                return;
            }
        };

        if let Some(timer_token) = surface.vblank_throttle_timer.take() {
            self.loop_handle.remove(timer_token);
        }

        let output = if let Some(output) = self.workspaces.get_current().space.outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: surface.device_id,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        let Some(frame_duration) = output
            .current_mode()
            .map(|mode| Duration::from_secs_f64(1_000f64 / mode.refresh as f64))
        else {
            return;
        };

        let tp = metadata.as_ref().and_then(|metadata| match metadata.time {
            smithay::backend::drm::DrmEventTime::Monotonic(tp) => tp.is_zero().not().then_some(tp),
            smithay::backend::drm::DrmEventTime::Realtime(_) => None,
        });

        let seq = metadata
            .as_ref()
            .map(|metadata| metadata.sequence)
            .unwrap_or(0);

        let (clock, flags) = if let Some(tp) = tp {
            (
                tp.into(),
                wp_presentation_feedback::Kind::Vsync
                    | wp_presentation_feedback::Kind::HwClock
                    | wp_presentation_feedback::Kind::HwCompletion,
            )
        } else {
            (self.clock.now(), wp_presentation_feedback::Kind::Vsync)
        };

        let vblank_remaining_time = surface
            .last_presentation_time
            .map(|last_presentation_time| {
                frame_duration.saturating_sub(Time::elapsed(&last_presentation_time, clock))
            });

        if let Some(vblank_remaining_time) = vblank_remaining_time {
            if vblank_remaining_time > frame_duration / 2 {
                static WARN_ONCE: Once = Once::new();
                WARN_ONCE.call_once(|| {
                    tracing::warn!("display running faster than expected, throttling vblanks and disabling HwClock")
                });
                let throttled_time = tp
                    .map(|tp| tp.saturating_add(vblank_remaining_time))
                    .unwrap_or(Duration::ZERO);
                let throttled_metadata = DrmEventMetadata {
                    sequence: seq,
                    time: DrmEventTime::Monotonic(throttled_time),
                };
                let timer_token = self
                    .loop_handle
                    .insert_source(
                        Timer::from_duration(vblank_remaining_time),
                        move |_, _, data| {
                            data.frame_finish(dev_id, crtc, &mut Some(throttled_metadata));
                            TimeoutAction::Drop
                        },
                    )
                    .expect("failed to register vblank throttle timer");
                surface.vblank_throttle_timer = Some(timer_token);
                return;
            }
        }
        surface.last_presentation_time = Some(clock);

        let submit_result = surface
            .drm_output
            .frame_submitted()
            .map_err(Into::<SwapBuffersError>::into);

        let schedule_render = match submit_result {
            Ok(user_data) => {
                if let Some(mut feedback) = user_data.flatten() {
                    feedback.presented(clock, Refresh::fixed(frame_duration), seq as u64, flags);
                }

                true
            }
            Err(err) => {
                tracing::warn!("Error during rendering: {:?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => true,
                    // If the device has been deactivated do not reschedule, this will be done
                    // by session resume
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
                        Some(DrmError::Access(DrmAccessError {
                            source,
                            ..
                        })) if source.kind() == io::ErrorKind::PermissionDenied
                    ),
                    SwapBuffersError::ContextLost(err) => panic!("Rendering loop lost: {err}"),
                }
            }
        };

        if schedule_render {
            let next_frame_target = clock + frame_duration;

            // What are we trying to solve by introducing a delay here:
            //
            // Basically it is all about latency of client provided buffers.
            // A client driven by frame callbacks will wait for a frame callback
            // to repaint and submit a new buffer. As we send frame callbacks
            // as part of the repaint in the compositor the latency would always
            // be approx. 2 frames. By introducing a delay before we repaint in
            // the compositor we can reduce the latency to approx. 1 frame + the
            // remaining duration from the repaint to the next VBlank.
            //
            // With the delay it is also possible to further reduce latency if
            // the client is driven by presentation feedback. As the presentation
            // feedback is directly sent after a VBlank the client can submit a
            // new buffer during the repaint delay that can hit the very next
            // VBlank, thus reducing the potential latency to below one frame.
            //
            // Choosing a good delay is a topic on its own so we just implement
            // a simple strategy here. We just split the duration between two
            // VBlanks into two steps, one for the client repaint and one for the
            // compositor repaint. Theoretically the repaint in the compositor should
            // be faster so we give the client a bit more time to repaint. On a typical
            // modern system the repaint in the compositor should not take more than 2ms
            // so this should be safe for refresh rates up to at least 120 Hz. For 120 Hz
            // this results in approx. 3.33ms time for repainting in the compositor.
            // A too big delay could result in missing the next VBlank in the compositor.
            //
            // A more complete solution could work on a sliding window analyzing past repaints
            // and do some prediction for the next repaint.
            let repaint_delay = Duration::from_secs_f64(frame_duration.as_secs_f64() * 0.6f64);

            let timer = if surface.render_node != self.backend_data.primary_gpu {
                // However, if we need to do a copy, that might not be enough.
                // (And without actual comparison to previous frames we cannot really know.)
                // So lets ignore that in those cases to avoid thrashing performance.
                tracing::trace!("scheduling repaint timer immediately on {:?}", crtc);
                Timer::immediate()
            } else {
                tracing::trace!(
                    "scheduling repaint timer with delay {:?} on {:?}",
                    repaint_delay,
                    crtc
                );
                Timer::from_duration(repaint_delay)
            };

            self.loop_handle
                .insert_source(timer, move |_, _, data| {
                    data.render(dev_id, Some(crtc), next_frame_target);
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        }
    }
    fn render(&mut self, node: DrmNode, crtc: Option<crtc::Handle>, frame_target: Time<Monotonic>) {
        let device_backend = match self.backend_data.devices.get_mut(&node) {
            Some(backend) => backend,
            None => {
                tracing::error!("Trying to render on non-existent backend {}", node);
                return;
            }
        };

        if let Some(crtc) = crtc {
            self.render_surface(node, crtc, frame_target);
        } else {
            let crtcs: Vec<_> = device_backend.surfaces.keys().copied().collect();
            for crtc in crtcs {
                self.render_surface(node, crtc, frame_target);
            }
        };
    }
    pub fn render_surface(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        frame_target: Time<Monotonic>,
    ) {
        let output = if let Some(output) = self.workspaces.get_current().space.outputs().find(|o| {
            o.user_data().get::<UdevOutputId>()
                == Some(&UdevOutputId {
                    device_id: node,
                    crtc,
                })
        }) {
            output.clone()
        } else {
            // somehow we got called with an invalid output
            return;
        };

        self.pre_repaint(&output, frame_target);
        let ws = self.workspaces.get_current();

        let device = if let Some(device) = self.backend_data.devices.get_mut(&node) {
            device
        } else {
            return;
        };

        let surface = if let Some(surface) = device.surfaces.get_mut(&crtc) {
            surface
        } else {
            return;
        };

        let start = Instant::now();

        let primary_gpu = self.backend_data.primary_gpu;
        let render_node = surface.render_node; //.unwrap_or(primary_gpu);
        let mut renderer = if primary_gpu == render_node {
            self.backend_data.gpus.single_renderer(&render_node)
        } else {
            let format = surface.drm_output.format();
            self.backend_data
                .gpus
                .renderer(&primary_gpu, &render_node, format)
        }
        .unwrap();

        let result = render_surface(
            ws,
            surface,
            self.pointer_location,
            &mut renderer,
            &self.config,
        );
        let reschedule = match result {
            Ok((has_rendered, states)) => {
                let dmabuf_feedback = surface.dmabuf_feedback.clone();
                self.post_repaint(&output, frame_target, dmabuf_feedback, &states);
                !has_rendered
            }
            Err(err) => {
                tracing::warn!("Error during rendering: {:#?}", err);
                match err {
                    SwapBuffersError::AlreadySwapped => false,
                    SwapBuffersError::TemporaryFailure(err) => match err.downcast_ref::<DrmError>()
                    {
                        Some(DrmError::DeviceInactive) => true,
                        Some(DrmError::Access(DrmAccessError { source, .. })) => {
                            source.kind() == io::ErrorKind::PermissionDenied
                        }
                        _ => false,
                    },
                    SwapBuffersError::ContextLost(err) => match err.downcast_ref::<DrmError>() {
                        Some(DrmError::TestFailed(_)) => {
                            // reset the complete state, disabling all connectors and planes in case we hit a test failed
                            // most likely we hit this after a tty switch when a foreign master changed CRTC <-> connector bindings
                            // and we run in a mismatch
                            device
                                .drm_output_manager
                                .device_mut()
                                .reset_state()
                                .expect("failed to reset drm device");
                            true
                        }
                        _ => panic!("Rendering loop lost: {err}"),
                    },
                }
            }
        };

        if reschedule {
            let output_refresh = match output.current_mode() {
                Some(mode) => mode.refresh,
                None => return,
            };

            // If reschedule is true we either hit a temporary failure or more likely rendering
            // did not cause any damage on the output. In this case we just re-schedule a repaint
            // after approx. one frame to re-test for damage.
            let next_frame_target =
                frame_target + Duration::from_millis(1_000_000 / output_refresh as u64);
            let reschedule_timeout =
                Duration::from(next_frame_target).saturating_sub(self.clock.now().into());
            tracing::trace!(
                "reschedule repaint timer with delay {:?} on {:?}",
                reschedule_timeout,
                crtc,
            );
            let timer = Timer::from_duration(reschedule_timeout);
            self.loop_handle
                .insert_source(timer, move |_, _, data| {
                    data.render(node, Some(crtc), next_frame_target);
                    TimeoutAction::Drop
                })
                .expect("failed to schedule frame timer");
        } else {
            let elapsed = start.elapsed();
            tracing::trace!(?elapsed, "rendered surface");
        }
    }
    pub fn pre_repaint(&mut self, output: &Output, frame_target: impl Into<Time<Monotonic>>) {
        let frame_target = frame_target.into();

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();
        self.workspaces
            .get_current()
            .space
            .elements()
            .for_each(|window| {
                window.with_surfaces(|surface, states| {
                    if let Some(mut commit_timer_state) = states
                        .data_map
                        .get::<CommitTimerBarrierStateUserData>()
                        .map(|commit_timer| commit_timer.lock().unwrap())
                    {
                        commit_timer_state.signal_until(frame_target);
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                });
            });

        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                if let Some(mut commit_timer_state) = states
                    .data_map
                    .get::<CommitTimerBarrierStateUserData>()
                    .map(|commit_timer| commit_timer.lock().unwrap())
                {
                    commit_timer_state.signal_until(frame_target);
                    let client = surface.client().unwrap();
                    clients.insert(client.id(), client);
                }
            });
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        //if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
        //    with_surfaces_surface_tree(surface, |surface, states| {
        //        if let Some(mut commit_timer_state) = states
        //            .data_map
        //            .get::<CommitTimerBarrierStateUserData>()
        //            .map(|commit_timer| commit_timer.lock().unwrap())
        //        {
        //            commit_timer_state.signal_until(frame_target);
        //            let client = surface.client().unwrap();
        //            clients.insert(client.id(), client);
        //        }
        //    });
        //}

        //if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
        //    with_surfaces_surface_tree(surface, |surface, states| {
        //        if let Some(mut commit_timer_state) = states
        //            .data_map
        //            .get::<CommitTimerBarrierStateUserData>()
        //            .map(|commit_timer| commit_timer.lock().unwrap())
        //        {
        //            commit_timer_state.signal_until(frame_target);
        //            let client = surface.client().unwrap();
        //            clients.insert(client.id(), client);
        //        }
        //    });
        //}

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client)
                .blocker_cleared(self, &dh);
        }
    }

    pub fn post_repaint(
        &mut self,
        output: &Output,
        time: impl Into<Duration>,
        dmabuf_feedback: Option<SurfaceDmabufFeedback>,
        render_element_states: &RenderElementStates,
    ) {
        let time = time.into();
        let throttle = Some(Duration::from_secs(1));

        #[allow(clippy::mutable_key_type)]
        let mut clients: HashMap<ClientId, Client> = HashMap::new();

        self.workspaces
            .get_current()
            .space
            .elements()
            .for_each(|window| {
                window.with_surfaces(|surface, states| {
                    let primary_scanout_output = surface_primary_scanout_output(surface, states);

                    if let Some(output) = primary_scanout_output.as_ref() {
                        with_fractional_scale(states, |fraction_scale| {
                            fraction_scale
                                .set_preferred_scale(output.current_scale().fractional_scale());
                        });
                    }

                    if primary_scanout_output
                        .as_ref()
                        .map(|o| o == output)
                        .unwrap_or(true)
                    {
                        let fifo_barrier = states
                            .cached_state
                            .get::<FifoBarrierCachedState>()
                            .current()
                            .barrier
                            .take();

                        if let Some(fifo_barrier) = fifo_barrier {
                            fifo_barrier.signal();
                            let client = surface.client().unwrap();
                            clients.insert(client.id(), client);
                        }
                    }
                });

                if self
                    .workspaces
                    .get_current()
                    .space
                    .outputs_for_element(window)
                    .contains(output)
                {
                    window.send_frame(output, time, throttle, surface_primary_scanout_output);
                    if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                        window.send_dmabuf_feedback(
                            output,
                            surface_primary_scanout_output,
                            |surface, _| {
                                select_dmabuf_feedback(
                                    surface,
                                    render_element_states,
                                    &dmabuf_feedback.render_feedback,
                                    &dmabuf_feedback.scanout_feedback,
                                )
                            },
                        );
                    }
                }
            });
        let map = smithay::desktop::layer_map_for_output(output);
        for layer_surface in map.layers() {
            layer_surface.with_surfaces(|surface, states| {
                let primary_scanout_output = surface_primary_scanout_output(surface, states);

                if let Some(output) = primary_scanout_output.as_ref() {
                    with_fractional_scale(states, |fraction_scale| {
                        fraction_scale
                            .set_preferred_scale(output.current_scale().fractional_scale());
                    });
                }

                if primary_scanout_output
                    .as_ref()
                    .map(|o| o == output)
                    .unwrap_or(true)
                {
                    let fifo_barrier = states
                        .cached_state
                        .get::<FifoBarrierCachedState>()
                        .current()
                        .barrier
                        .take();

                    if let Some(fifo_barrier) = fifo_barrier {
                        fifo_barrier.signal();
                        let client = surface.client().unwrap();
                        clients.insert(client.id(), client);
                    }
                }
            });

            layer_surface.send_frame(output, time, throttle, surface_primary_scanout_output);
            if let Some(dmabuf_feedback) = dmabuf_feedback.as_ref() {
                layer_surface.send_dmabuf_feedback(
                    output,
                    surface_primary_scanout_output,
                    |surface, _| {
                        select_dmabuf_feedback(
                            surface,
                            render_element_states,
                            &dmabuf_feedback.render_feedback,
                            &dmabuf_feedback.scanout_feedback,
                        )
                    },
                );
            }
        }
        // Drop the lock to the layer map before calling blocker_cleared, which might end up
        // calling the commit handler which in turn again could access the layer map.
        std::mem::drop(map);

        //if let CursorImageStatus::Surface(ref surface) = self.cursor_status {
        //    with_surfaces_surface_tree(surface, |surface, states| {
        //        let primary_scanout_output = surface_primary_scanout_output(surface, states);

        //        if let Some(output) = primary_scanout_output.as_ref() {
        //            with_fractional_scale(states, |fraction_scale| {
        //                fraction_scale
        //                    .set_preferred_scale(output.current_scale().fractional_scale());
        //            });
        //        }

        //        if primary_scanout_output
        //            .as_ref()
        //            .map(|o| o == output)
        //            .unwrap_or(true)
        //        {
        //            let fifo_barrier = states
        //                .cached_state
        //                .get::<FifoBarrierCachedState>()
        //                .current()
        //                .barrier
        //                .take();

        //            if let Some(fifo_barrier) = fifo_barrier {
        //                fifo_barrier.signal();
        //                let client = surface.client().unwrap();
        //                clients.insert(client.id(), client);
        //            }
        //        }
        //    });
        //}

        //if let Some(surface) = self.dnd_icon.as_ref().map(|icon| &icon.surface) {
        //    with_surfaces_surface_tree(surface, |surface, states| {
        //        let primary_scanout_output = surface_primary_scanout_output(surface, states);

        //        if let Some(output) = primary_scanout_output.as_ref() {
        //            with_fractional_scale(states, |fraction_scale| {
        //                fraction_scale
        //                    .set_preferred_scale(output.current_scale().fractional_scale());
        //            });
        //        }

        //        if primary_scanout_output
        //            .as_ref()
        //            .map(|o| o == output)
        //            .unwrap_or(true)
        //        {
        //            let fifo_barrier = states
        //                .cached_state
        //                .get::<FifoBarrierCachedState>()
        //                .current()
        //                .barrier
        //                .take();

        //            if let Some(fifo_barrier) = fifo_barrier {
        //                fifo_barrier.signal();
        //                let client = surface.client().unwrap();
        //                clients.insert(client.id(), client);
        //            }
        //        }
        //    });
        //}

        let dh = self.display_handle.clone();
        for client in clients.into_values() {
            self.client_compositor_state(&client)
                .blocker_cleared(self, &dh);
        }
    }
}
#[derive(Debug, Clone)]
pub struct SurfaceDmabufFeedback {
    pub render_feedback: DmabufFeedback,
    pub scanout_feedback: DmabufFeedback,
}

pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<Window>,
    render_element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

    space.elements().for_each(|window| {
        if space.outputs_for_element(window).contains(output) {
            window.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| {
                    surface_presentation_feedback_flags_from_states(surface, render_element_states)
                },
            );
        }
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.take_presentation_feedback(
            &mut output_presentation_feedback,
            surface_primary_scanout_output,
            |surface, _| {
                surface_presentation_feedback_flags_from_states(surface, render_element_states)
            },
        );
    }

    output_presentation_feedback
}

pub fn render_surface<'a>(
    ws: &Workspace,
    surface: &'a mut Surface,
    pointer_location: Point<f64, Logical>,
    renderer: &mut GlMultiRenderer<'a>,
    config: &Config,
) -> Result<(bool, RenderElementStates), SwapBuffersError> {
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
            pointer_location.to_physical(scale),
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
                renderer,
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
    let border = &config.border;
    let active = ws.active_window.as_ref();
    let fullscreen = is_fullscreen(ws.space.elements());

    if let Some(win) = fullscreen {
        let loc = ws.space.element_location(win).unwrap();
        for elem in win.render_elements(renderer, loc.to_physical(physical_scale), scale, 1.0) {
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

            //let border_elem =
            //    BorderShader::element(renderer, border_geo, 1.0, &color, border.thickness as f32);

            //elements.push(CustomRenderElements::Shader(border_elem));

            // Window content
            let offset = loc - win_geo.loc;
            for elem in
                window.render_elements(renderer, offset.to_physical(physical_scale), scale, 1.0)
            {
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
                renderer,
                geo.loc.to_physical_precise_round(physical_scale),
                scale,
                1.0,
            ) {
                elements.push(CustomRenderElements::Window(elem));
            }
        }
    }

    let frame_mode = FrameFlags::DEFAULT;
    let (rendered, states) = surface
        .drm_output
        .render_frame(renderer, &elements, CLEAR_COLOR, frame_mode)
        .map(|render_frame_result| {
            #[cfg(feature = "renderer_sync")]
            if let PrimaryPlaneElement::Swapchain(element) = render_frame_result.primary_element {
                element.sync.wait();
            }
            (!render_frame_result.is_empty, render_frame_result.states)
        })
        .map_err(|err| match err {
            smithay::backend::drm::compositor::RenderFrameError::PrepareFrame(err) => {
                SwapBuffersError::from(err)
            }
            smithay::backend::drm::compositor::RenderFrameError::RenderFrame(
                smithay::backend::renderer::damage::Error::Rendering(err),
            ) => SwapBuffersError::from(err),
            _ => unreachable!(),
        })?;

    update_primary_scanout_output(&ws.space, output, &states);

    if rendered {
        let output_presentation_feedback = take_presentation_feedback(output, &ws.space, &states);
        surface
            .drm_output
            .queue_frame(Some(output_presentation_feedback))
            .map_err(Into::<SwapBuffersError>::into)?;
    }

    Ok((rendered, states))
}

pub fn update_primary_scanout_output(
    space: &Space<Window>,
    output: &Output,
    render_element_states: &RenderElementStates,
) {
    space.elements().for_each(|window| {
        window.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.with_surfaces(|surface, states| {
            update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );
        });
    }
}

fn get_surface_dmabuf_feedback(
    primary_gpu: DrmNode,
    render_node: Option<DrmNode>,
    scanout_node: DrmNode,
    gpus: &mut GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    surface: &DrmSurface,
) -> Option<SurfaceDmabufFeedback> {
    let primary_formats = gpus.single_renderer(&primary_gpu).ok()?.dmabuf_formats();
    let render_formats = if let Some(render_node) = render_node {
        gpus.single_renderer(&render_node).ok()?.dmabuf_formats()
    } else {
        FormatSet::default()
    };

    let all_render_formats = primary_formats
        .iter()
        .chain(render_formats.iter())
        .copied()
        .collect::<FormatSet>();

    let planes = surface.planes().clone();

    // We limit the scan-out tranche to formats we can also render from
    // so that there is always a fallback render path available in case
    // the supplied buffer can not be scanned out directly
    let planes_formats = surface
        .plane_info()
        .formats
        .iter()
        .copied()
        .chain(planes.overlay.into_iter().flat_map(|p| p.formats))
        .collect::<FormatSet>()
        .intersection(&all_render_formats)
        .copied()
        .collect::<FormatSet>();

    let builder = DmabufFeedbackBuilder::new(primary_gpu.dev_id(), primary_formats);
    let render_feedback = if let Some(render_node) = render_node {
        builder
            .clone()
            .add_preference_tranche(render_node.dev_id(), None, render_formats.clone())
            .build()
            .unwrap()
    } else {
        builder.clone().build().unwrap()
    };

    let scanout_feedback = builder
        .add_preference_tranche(
            surface.device_fd().dev_id().unwrap(),
            Some(zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout),
            planes_formats,
        )
        .add_preference_tranche(scanout_node.dev_id(), None, render_formats)
        .build()
        .unwrap();

    Some(SurfaceDmabufFeedback {
        render_feedback,
        scanout_feedback,
    })
}

impl DrmLeaseHandler for State {
    fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState {
        self.backend_data
            .devices
            .get_mut(&node)
            .unwrap()
            .leasing_global
            .as_mut()
            .unwrap()
    }

    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected> {
        let backend = self
            .backend_data
            .devices
            .get(&node)
            .ok_or(LeaseRejected::default())?;

        let drm_device = backend.drm_output_manager.device();
        let mut builder = DrmLeaseBuilder::new(drm_device);
        for conn in request.connectors {
            if let Some((_, crtc)) = backend
                .non_desktop_connectors
                .iter()
                .find(|(handle, _)| *handle == conn)
            {
                builder.add_connector(conn);
                builder.add_crtc(*crtc);
                let planes = drm_device.planes(crtc).map_err(LeaseRejected::with_cause)?;
                let (primary_plane, primary_plane_claim) = planes
                    .primary
                    .iter()
                    .find_map(|plane| {
                        drm_device
                            .claim_plane(plane.handle, *crtc)
                            .map(|claim| (plane, claim))
                    })
                    .ok_or_else(LeaseRejected::default)?;
                builder.add_plane(primary_plane.handle, primary_plane_claim);
                if let Some((cursor, claim)) = planes.cursor.iter().find_map(|plane| {
                    drm_device
                        .claim_plane(plane.handle, *crtc)
                        .map(|claim| (plane, claim))
                }) {
                    builder.add_plane(cursor.handle, claim);
                }
            } else {
                tracing::warn!(
                    ?conn,
                    "Lease requested for desktop connector, denying request"
                );
                return Err(LeaseRejected::default());
            }
        }

        Ok(builder)
    }

    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) {
        let backend = self.backend_data.devices.get_mut(&node).unwrap();
        backend.active_leases.push(lease);
    }

    fn lease_destroyed(&mut self, node: DrmNode, lease: u32) {
        let backend = self.backend_data.devices.get_mut(&node).unwrap();
        backend.active_leases.retain(|l| l.id() != lease);
    }
}

delegate_drm_lease!(State);

impl DrmSyncobjHandler for State {
    fn drm_syncobj_state(&mut self) -> Option<&mut DrmSyncobjState> {
        self.backend_data.syncobj_state.as_mut()
    }
}
smithay::delegate_drm_syncobj!(State);
