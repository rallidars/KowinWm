mod device;
mod surface;

use std::{collections::HashMap, io, path::PathBuf, time::Duration};

use crate::{state::State, udev::device::Device};
use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        drm::{self, DrmDeviceFd, DrmNode, NodeType},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            gles::GlesRenderer,
            multigpu::{gbm::GbmGlesBackend, GpuManager},
            ImportDma, ImportEgl,
        },
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{self, UdevBackend, UdevEvent},
        SwapBuffersError,
    },
    delegate_dmabuf,
    desktop::{layer_map_for_output, space::SpaceElement},
    reexports::{
        calloop::EventLoop,
        input::Libinput,
        wayland_server::{protocol::wl_surface, Display},
    },
    wayland::dmabuf::{
        DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier,
    },
};
use smithay_drm_extras::drm_scanner::DrmScanEvent;

pub struct UdevData {
    pub session: LibSeatSession,
    primary_gpu: DrmNode,
    gpus: GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>,
    devices: HashMap<DrmNode, Device>,
    dmabuf_state: Option<(DmabufState, DmabufGlobal)>,
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
    };

    /*
     * Initialize libinput state
     */

    let mut state = State::new(event_loop.handle(), event_loop.get_signal(), display, data);

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
                tracing::info!("pausing session");

                for backend in data.backend_data.devices.values_mut() {
                    backend.drm_output_manager.pause();
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
                    //TODO handle errors
                    for (crtc, surface) in backend
                        .surfaces
                        .iter_mut()
                        .map(|(handle, surface)| (*handle, surface))
                    {
                        backend
                            .drm_output_manager
                            .activate(false)
                            .expect("failed to activate drm backend");
                        data.loop_handle.insert_idle(move |data| {
                            if let Some(SwapBuffersError::ContextLost(_)) =
                                data.render(node, crtc).err()
                            {
                                tracing::info!("Context lost on device {}, re-creating", node);
                                data.on_device_removed(node);
                                data.on_device_added(node, node.dev_path().unwrap());
                            }
                        });
                    }
                }
            }
        })
        .unwrap();
    /*
     * Initialize udev
     */

    let backend = UdevBackend::new(&state.backend_data.session.seat()).unwrap();
    for (device_id, path) in backend.device_list() {
        tracing::info!("udev device {}", path.display());
        state.on_udev_event(UdevEvent::Added {
            device_id,
            path: path.to_owned(),
        });
    }

    event_loop
        .handle()
        .insert_source(backend, |event, _, calloopdata| {
            calloopdata.on_udev_event(event)
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
    match renderer.bind_wl_display(&state.display_handle) {
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
        .create_global_with_default_feedback::<State>(&state.display_handle, &default_feedback);
    state.backend_data.dmabuf_state = Some((dmabuf_state, global));

    let autostart = state.config.autostart.clone();

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

    for program in autostart {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&program)
            .spawn()
            .map_err(|e| tracing::info!("Failed to spawn '{program}': {e}"))
            .ok();
    }

    event_loop
        .run(None, &mut state, move |data| {
            for ws in data.workspaces.workspaces.iter() {
                ws.space.elements().for_each(|e| e.refresh());
            }

            let output = data
                .workspaces
                .get_current()
                .space
                .outputs()
                .next()
                .unwrap();
            for layer in layer_map_for_output(output).layers() {
                layer.send_frame(
                    output,
                    data.start_time.elapsed(),
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            }

            data.display_handle.flush_clients().unwrap();
            data.popup_manager.cleanup();
        })
        .unwrap();
}

// Udev
impl State {
    pub fn on_udev_event(&mut self, event: UdevEvent) {
        match event {
            UdevEvent::Added { device_id, path } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_added(node, path);
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_changed(node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.on_device_removed(node);
                }
            }
        }
    }
}

// Drm
impl State {
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
                surface.drm_output.frame_submitted().ok();
                tracing::debug!("VBlank event on {:?}", crtc);
                self.render(node, crtc).unwrap();
            }
            drm::DrmEvent::Error(_) => {}
        }
    }

    pub fn on_connector_event(&mut self, node: DrmNode, event: DrmScanEvent) {
        match event {
            DrmScanEvent::Connected {
                connector,
                crtc: Some(crtc),
            } => {
                self.connected(connector, crtc, node);
            }
            DrmScanEvent::Disconnected {
                crtc: Some(crtc), ..
            } => {
                let device = if let Some(device) = self.backend_data.devices.get_mut(&node) {
                    device
                } else {
                    tracing::error!("Received connector event for unknown device: {:?}", node);
                    return;
                };
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
