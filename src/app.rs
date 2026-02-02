use std::{ffi::CString, sync::Arc, thread, time::Duration};

use raw_window_handle::HasWindowHandle;
use windows::Win32::{
    Foundation::{HMODULE, HWND},
    Graphics::{
        Direct3D::{
            D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_11_1, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            Fxc::{D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile},
        },
        Direct3D11::{
            D3D11_COMPARISON_FUNC, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION,
            D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11Device,
            ID3D11DeviceContext, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11SamplerState,
            ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader,
        },
        Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
        Dxgi::{
            DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
            DXGI_SWAP_CHAIN_FULLSCREEN_DESC, DXGI_SWAP_EFFECT_FLIP_DISCARD,
            DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIDevice, IDXGIFactory2,
            IDXGIKeyedMutex, IDXGISwapChain1,
        },
    },
    UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
        WDA_EXCLUDEFROMCAPTURE, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    },
};
use windows::core::{Interface, PCSTR};
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

use crate::capture::{CaptureBuffer, Capturer, SharedHandle};

const SHADER_SOURCE: &str = include_str!("shader.hlsl");

struct App {
    window: Arc<Window>,
    capture_buffer: CaptureBuffer,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swapchain: IDXGISwapChain1,
    rtv: ID3D11RenderTargetView,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    sampler: ID3D11SamplerState,
    shared_handle: Option<SharedHandle>,
    shared_texture: Option<ID3D11Texture2D>,
    shared_srv: Option<ID3D11ShaderResourceView>,
    shared_mutex: Option<IDXGIKeyedMutex>,
    shared_size: (u32, u32),
    last_frame_id: u64,
    size: winit::dpi::PhysicalSize<u32>,
}

impl App {
    async fn new(window: Arc<Window>, capture_buffer: CaptureBuffer) -> Self {
        let size = window.inner_size();
        let hwnd = window_to_hwnd(&window).expect("Failed to get HWND");
        let (device, context) = create_d3d_device().expect("Failed to create D3D11 device");
        let swapchain = create_swapchain(&device, hwnd, size).expect("Failed to create swapchain");
        let rtv = create_render_target_view(&device, &swapchain)
            .expect("Failed to create render target view");
        let (vs, ps) = create_shaders(&device).expect("Failed to create shaders");
        let sampler = create_sampler(&device).expect("Failed to create sampler");

        let app = Self {
            window,
            capture_buffer,
            device,
            context,
            swapchain,
            rtv,
            vs,
            ps,
            sampler,
            shared_handle: None,
            shared_texture: None,
            shared_srv: None,
            shared_mutex: None,
            shared_size: (0, 0),
            last_frame_id: 0,
            size,
        };
        app.set_viewport(size);
        app
    }

    fn set_viewport(&self, size: winit::dpi::PhysicalSize<u32>) {
        let viewport = D3D11_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: size.width as f32,
            Height: size.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        unsafe {
            self.context.RSSetViewports(Some(&[viewport]));
        }
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }
        self.size = new_size;
        unsafe {
            let _ = self.swapchain.ResizeBuffers(
                0,
                new_size.width,
                new_size.height,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_SWAP_CHAIN_FLAG(0),
            );
        }
        self.rtv = create_render_target_view(&self.device, &self.swapchain).expect("RTV resize");
        self.set_viewport(new_size);
    }

    fn render(&mut self) {
        let (handle, width, height, frame_id) = {
            let shared = self.capture_buffer.lock().unwrap();
            (shared.handle, shared.width, shared.height, shared.frame_id)
        };
        let Some(handle) = handle else {
            return;
        };
        if frame_id == self.last_frame_id {
            return;
        }
        if (self.shared_handle != Some(handle) || self.shared_size != (width, height))
            && let Err(err) = self.open_shared_texture(handle, width, height)
        {
            eprintln!("Failed to open shared texture: {err:?}");
            return;
        }
        let Some(shared_srv) = &self.shared_srv else {
            return;
        };
        let Some(shared_mutex) = &self.shared_mutex else {
            return;
        };

        if unsafe { shared_mutex.AcquireSync(1, 0) }.is_err() {
            return;
        }

        unsafe {
            self.context
                .OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
            self.context
                .ClearRenderTargetView(&self.rtv, &[0.0, 0.0, 0.0, 0.0]);
            self.context.IASetInputLayout(None);
            self.context
                .IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            self.context.VSSetShader(&self.vs, None);
            self.context.PSSetShader(&self.ps, None);
            self.context
                .PSSetShaderResources(0, Some(&[Some(shared_srv.clone())]));
            self.context
                .PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            self.context.Draw(3, 0);
        }

        let _ = unsafe { shared_mutex.ReleaseSync(0) };
        let _ = unsafe { self.swapchain.Present(0, DXGI_PRESENT(0)) };
        self.last_frame_id = frame_id;
    }

    fn open_shared_texture(
        &mut self,
        handle: SharedHandle,
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device.OpenSharedResource(handle.0, &mut texture)?;
        }
        let texture = texture.ok_or_else(|| anyhow::anyhow!("Shared texture missing"))?;
        let mutex: IDXGIKeyedMutex = texture.cast()?;
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut srv))?;
        }
        self.shared_texture = Some(texture);
        self.shared_mutex = Some(mutex);
        self.shared_srv = srv;
        self.shared_handle = Some(handle);
        self.shared_size = (width, height);
        Ok(())
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

        let primary_monitor = Monitor::primary().unwrap();
        let capture_buffer = CaptureBuffer::default();
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
            capture_buffer.clone(),
        );
        thread::Builder::new()
            .name("capture".to_string())
            .spawn(move || {
                Capturer::start(settings).unwrap();
            })
            .unwrap();

        self.app = Some(pollster::block_on(App::new(
            Arc::new(window),
            capture_buffer,
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

fn window_to_hwnd(window: &Window) -> anyhow::Result<HWND> {
    let raw_handle = window.window_handle()?.as_raw();
    let hwnd = match raw_handle {
        raw_window_handle::RawWindowHandle::Win32(handle) => {
            HWND(handle.hwnd.get() as *mut std::ffi::c_void)
        }
        _ => return Err(anyhow::anyhow!("Not a Windows handle")),
    };
    Ok(hwnd)
}

fn create_d3d_device() -> anyhow::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL::default();
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
    }
    let device = device.ok_or_else(|| anyhow::anyhow!("Failed to create D3D11 device"))?;
    let context = context.ok_or_else(|| anyhow::anyhow!("Failed to create D3D11 context"))?;
    Ok((device, context))
}

fn create_swapchain(
    device: &ID3D11Device,
    hwnd: HWND,
    size: winit::dpi::PhysicalSize<u32>,
) -> anyhow::Result<IDXGISwapChain1> {
    let dxgi_device: IDXGIDevice = device.cast()?;
    let adapter: IDXGIAdapter = unsafe { dxgi_device.GetAdapter()? };
    let factory: IDXGIFactory2 = unsafe { adapter.GetParent()? };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: size.width,
        Height: size.height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: 0,
    };
    let swapchain = unsafe {
        factory.CreateSwapChainForHwnd(
            device,
            hwnd,
            &desc,
            Option::<*const DXGI_SWAP_CHAIN_FULLSCREEN_DESC>::None,
            Option::<&windows::Win32::Graphics::Dxgi::IDXGIOutput>::None,
        )?
    };
    Ok(swapchain)
}

fn create_render_target_view(
    device: &ID3D11Device,
    swapchain: &IDXGISwapChain1,
) -> anyhow::Result<ID3D11RenderTargetView> {
    let back_buffer: ID3D11Texture2D = unsafe { swapchain.GetBuffer(0)? };
    let mut rtv = None;
    unsafe {
        device.CreateRenderTargetView(&back_buffer, None, Some(&mut rtv))?;
    }
    rtv.ok_or_else(|| anyhow::anyhow!("Failed to create render target view"))
}

fn create_shaders(
    device: &ID3D11Device,
) -> anyhow::Result<(ID3D11VertexShader, ID3D11PixelShader)> {
    let vs_blob = compile_shader(SHADER_SOURCE, "vs_main", "vs_5_0")?;
    let ps_blob = compile_shader(SHADER_SOURCE, "ps_main", "ps_5_0")?;
    let mut vs = None;
    let mut ps = None;
    unsafe {
        device.CreateVertexShader(blob_bytes(&vs_blob), None, Some(&mut vs))?;
        device.CreatePixelShader(blob_bytes(&ps_blob), None, Some(&mut ps))?;
    }
    let vs = vs.ok_or_else(|| anyhow::anyhow!("Failed to create vertex shader"))?;
    let ps = ps.ok_or_else(|| anyhow::anyhow!("Failed to create pixel shader"))?;
    Ok((vs, ps))
}

fn create_sampler(device: &ID3D11Device) -> anyhow::Result<ID3D11SamplerState> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_FUNC(0),
        BorderColor: [0.0, 0.0, 0.0, 0.0],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut sampler = None;
    unsafe {
        device.CreateSamplerState(&desc, Some(&mut sampler))?;
    }
    sampler.ok_or_else(|| anyhow::anyhow!("Failed to create sampler"))
}

fn compile_shader(
    source: &str,
    entry: &str,
    target: &str,
) -> anyhow::Result<windows::Win32::Graphics::Direct3D::ID3DBlob> {
    let entry_c = CString::new(entry)?;
    let target_c = CString::new(target)?;
    let mut shader = None;
    let mut error = None;
    let flags = D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3;
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            None,
            None,
            None,
            PCSTR::from_raw(entry_c.as_ptr() as *const u8),
            PCSTR::from_raw(target_c.as_ptr() as *const u8),
            flags,
            0,
            &mut shader,
            Some(&mut error),
        )
    };
    if let Err(err) = result {
        let message = error
            .as_ref()
            .map(blob_to_string)
            .unwrap_or_else(|| "<no shader error>".to_string());
        return Err(anyhow::anyhow!("D3DCompile failed: {err:?} {message}"));
    }
    shader.ok_or_else(|| anyhow::anyhow!("Shader blob missing"))
}

fn blob_bytes(blob: &windows::Win32::Graphics::Direct3D::ID3DBlob) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

fn blob_to_string(blob: &windows::Win32::Graphics::Direct3D::ID3DBlob) -> String {
    let bytes = blob_bytes(blob);
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn apply_click_through(window: &Window) -> anyhow::Result<()> {
    let hwnd = window_to_hwnd(window)?;
    unsafe {
        let current_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_style = current_style
            | WS_EX_LAYERED.0 as isize
            | WS_EX_TRANSPARENT.0 as isize
            | WS_EX_TOPMOST.0 as isize
            | WS_EX_TOOLWINDOW.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        )?;
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)?;
    }
    Ok(())
}
