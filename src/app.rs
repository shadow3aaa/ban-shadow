use std::{sync::Arc, thread, time::Duration};

use raw_window_handle::HasWindowHandle;
use wgpu::{RenderPipelineDescriptor, include_wgsl};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
        WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    },
};
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings as CaptureSettings,
};
use windows_capture::{capture::GraphicsCaptureApiHandler, monitor::Monitor};
use winit::{
    application::ApplicationHandler,
    platform::windows::WindowAttributesExtWindows,
    window::{Window, WindowAttributes},
};

use crate::capture::{CaptureBuffer, Capturer};

struct App {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    // Rendering resources
    last_frame_id: u64,
    local_buffer: Vec<u8>,
    capture_buffer: CaptureBuffer,
    render_pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    bind_group_layout: wgpu::BindGroupLayout,
    texture: wgpu::Texture,
}

impl App {
    async fn new(window: Arc<Window>, capture_buffer: CaptureBuffer) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .unwrap();
        let surface_caps = surface.get_capabilities(&adapter);
        let surface_format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb()) // Prefer sRGB format
            .unwrap_or(surface_caps.formats[0]);
        let alpha_mode = surface_caps
            .alpha_modes
            .iter()
            .find(|&&m| m == wgpu::CompositeAlphaMode::PreMultiplied)
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Immediate,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 1, // Minimize latency
        };
        let texture_size = wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Overlay Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb, // Match capture format
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(include_wgsl!("shader.wgsl"));
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    // Texture
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    // Sampler
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
            label: Some("texture_bind_group_layout"),
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("texture_bind_group"),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            ..Default::default()
        });
        let render_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let local_buffer = Vec::new();

        Self {
            window,
            surface,
            device,
            queue,
            config,
            size,
            local_buffer,
            capture_buffer,
            render_pipeline,
            bind_group,
            bind_group_layout,
            texture,
            last_frame_id: 0,
        }
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
            // Recreate texture with new size
            let texture_size = wgpu::Extent3d {
                width: new_size.width,
                height: new_size.height,
                depth_or_array_layers: 1,
            };
            self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Overlay Texture"),
                size: texture_size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            // Update bind group to use the new texture view
            let texture_view = self
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.device.create_sampler(
                            &wgpu::SamplerDescriptor {
                                address_mode_u: wgpu::AddressMode::ClampToEdge,
                                address_mode_v: wgpu::AddressMode::ClampToEdge,
                                address_mode_w: wgpu::AddressMode::ClampToEdge,
                                mag_filter: wgpu::FilterMode::Linear,
                                min_filter: wgpu::FilterMode::Linear,
                                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                                ..Default::default()
                            },
                        )),
                    },
                ],
                label: Some("texture_bind_group"),
            });
        }
    }

    fn render(&mut self) {
        // Upload captured frame to texture
        {
            let mut shared = self.capture_buffer.lock().unwrap();
            if shared.frame_id > self.last_frame_id && !shared.buffer.is_empty() {
                if self.local_buffer.len() != shared.buffer.len() {
                    self.local_buffer.resize(shared.buffer.len(), 0);
                }
                std::mem::swap(&mut shared.buffer, &mut self.local_buffer);
                self.last_frame_id = shared.frame_id;
            }
        }
        let expected_size = (self.size.width * self.size.height * 4) as usize;
        if self.local_buffer.len() != expected_size {
            return; // Skip if buffer size is not initialized yet
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfoBase {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.local_buffer,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * self.size.width),
                rows_per_image: Some(self.size.height),
            },
            wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
        );

        // Render to the surface
        let output = self.surface.get_current_texture().unwrap();
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.draw(0..3, 0..1); // Fullscreen triangle
        }
        self.queue.submit(Some(encoder.finish()));
        output.present();
    }
}

#[derive(Default)]
pub struct AppHandler {
    app: Option<App>,
}

impl ApplicationHandler for AppHandler {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.app.is_some() {
            return;
        }
        // Create the overlay window
        let window = event_loop
            .create_window(
                WindowAttributes::default()
                    .with_title("Ban-Shadow Overlay")
                    .with_decorations(false)
                    .with_transparent(true)
                    .with_resizable(false)
                    .with_skip_taskbar(true)
                    .with_fullscreen(Some(winit::window::Fullscreen::Borderless(None))),
            )
            .unwrap();
        apply_click_through(&window).unwrap();

        // Launch capture thread
        let primary_monitor = Monitor::primary().unwrap(); // TODO: select monitor based on args
        let capature_buffer = CaptureBuffer::default();
        let settings = CaptureSettings::new(
            primary_monitor,
            CursorCaptureSettings::WithoutCursor,
            DrawBorderSettings::WithoutBorder,
            SecondaryWindowSettings::Exclude,
            MinimumUpdateIntervalSettings::Custom(
                Duration::from_secs(1) / primary_monitor.refresh_rate().unwrap(),
            ),
            DirtyRegionSettings::Default,
            ColorFormat::Bgra8,
            capature_buffer.clone(),
        );
        thread::Builder::new()
            .name("capture".to_string())
            .spawn(move || {
                Capturer::start(settings).unwrap();
            })
            .unwrap();

        self.app = Some(pollster::block_on(App::new(
            Arc::new(window),
            capature_buffer,
        )));
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        let _ = (event_loop, window_id);
        let Some(ref mut app) = self.app else {
            return;
        };
        match event {
            winit::event::WindowEvent::RedrawRequested => {
                app.render();
                self.app.as_mut().unwrap().window.request_redraw();
            }
            winit::event::WindowEvent::Resized(physical_size) => {
                app.resize(physical_size);
            }
            _ => {}
        }
    }
}

fn apply_click_through(window: &Window) -> anyhow::Result<()> {
    // Get the HWND from the winit window
    let raw_handle = window.window_handle()?.as_raw();
    let hwnd: HWND = match raw_handle {
        raw_window_handle::RawWindowHandle::Win32(handle) => {
            HWND(handle.hwnd.get() as *mut std::ffi::c_void)
        }
        _ => return Err(anyhow::anyhow!("Not a Windows handle")),
    };
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_style = current_style
            | WS_EX_LAYERED.0 as isize // Make layered
            | WS_EX_TRANSPARENT.0 as isize // Make click-through
            | WS_EX_TOPMOST.0 as isize // Force topmost
            | WS_EX_TOOLWINDOW.0 as isize; // Don't show in alt-tab
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST), // Place window at the top
            0,
            0,
            0,
            0, // Ignored since we're not changing size/position
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )?;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
    }
    Ok(())
}
