use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource};
use windows::core::Interface;
use windows_capture::capture::GraphicsCaptureApiHandler;
use windows_capture::settings::ColorFormat;

pub type CaptureBuffer = Arc<Mutex<SharedData>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SharedHandle(pub HANDLE);

unsafe impl Send for SharedHandle {}
unsafe impl Sync for SharedHandle {}

#[derive(Default)]
pub struct SharedData {
    pub handle: Option<SharedHandle>,
    pub width: u32,
    pub height: u32,
    pub frame_id: u64,
}

pub struct Capturer {
    shared_buffer: CaptureBuffer,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    shared_texture: Option<ID3D11Texture2D>,
    shared_mutex: Option<IDXGIKeyedMutex>,
    shared_size: (u32, u32),
    current_frame_id: u64,
}

impl GraphicsCaptureApiHandler for Capturer {
    type Flags = CaptureBuffer;
    type Error = anyhow::Error;

    fn new(ctx: windows_capture::capture::Context<Self::Flags>) -> Result<Self, Self::Error> {
        let shared_buffer = ctx.flags;
        Ok(Self {
            shared_buffer,
            device: ctx.device,
            context: ctx.device_context,
            shared_texture: None,
            shared_mutex: None,
            shared_size: (0, 0),
            current_frame_id: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut windows_capture::frame::Frame,
        capture_control: windows_capture::graphics_capture_api::InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let _ = capture_control;
        self.ensure_shared_texture(frame)?;
        let Some(shared_texture) = &self.shared_texture else {
            return Ok(());
        };
        let Some(shared_mutex) = &self.shared_mutex else {
            return Ok(());
        };

        if unsafe { shared_mutex.AcquireSync(0, 0) }.is_err() {
            return Ok(());
        }

        unsafe {
            self.context
                .CopyResource(shared_texture, frame.as_raw_texture());
        }

        let _ = unsafe { shared_mutex.ReleaseSync(1) };

        self.current_frame_id += 1;
        let mut shared = self.shared_buffer.lock().unwrap();
        shared.frame_id = self.current_frame_id;
        Ok(())
    }
}

impl Capturer {
    fn ensure_shared_texture(
        &mut self,
        frame: &windows_capture::frame::Frame,
    ) -> Result<(), anyhow::Error> {
        let width = frame.width();
        let height = frame.height();
        if self.shared_texture.is_some() && self.shared_size == (width, height) {
            return Ok(());
        }

        let format = dxgi_format_from_color(frame.color_format());
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
        };

        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        let texture = texture.ok_or_else(|| anyhow::anyhow!("Failed to create shared texture"))?;

        let dxgi_resource: IDXGIResource = texture.cast()?;
        let handle = unsafe { dxgi_resource.GetSharedHandle()? };

        let keyed_mutex: IDXGIKeyedMutex = texture.cast()?;

        {
            let mut shared = self.shared_buffer.lock().unwrap();
            shared.handle = Some(SharedHandle(handle));
            shared.width = width;
            shared.height = height;
            shared.frame_id = 0;
        }

        self.shared_texture = Some(texture);
        self.shared_mutex = Some(keyed_mutex);
        self.shared_size = (width, height);

        Ok(())
    }
}

fn dxgi_format_from_color(format: ColorFormat) -> DXGI_FORMAT {
    match format {
        ColorFormat::Rgba16F => DXGI_FORMAT_R16G16B16A16_FLOAT,
        ColorFormat::Rgba8 => DXGI_FORMAT_R8G8B8A8_UNORM,
        ColorFormat::Bgra8 => DXGI_FORMAT_B8G8R8A8_UNORM,
    }
}
