use std::sync::{Arc, Mutex};

use windows_capture::capture::GraphicsCaptureApiHandler;

pub type CaptureBuffer = Arc<Mutex<SharedData>>;

#[derive(Default)]
pub struct SharedData {
    pub buffer: Vec<u8>,
    pub frame_id: u64,
}

pub struct Capturer {
    shared_buffer: CaptureBuffer,
    scratch_buffer: Vec<u8>,
    current_frame_id: u64,
}

impl GraphicsCaptureApiHandler for Capturer {
    type Flags = CaptureBuffer;
    type Error = anyhow::Error;

    fn new(ctx: windows_capture::capture::Context<Self::Flags>) -> Result<Self, Self::Error> {
        let shared_buffer = ctx.flags;
        let scratch_buffer = Vec::new();
        Ok(Self {
            shared_buffer,
            scratch_buffer,
            current_frame_id: 0,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut windows_capture::frame::Frame,
        capture_control: windows_capture::graphics_capture_api::InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let _ = capture_control;
        let mut buffer = frame.buffer()?;
        let frame_data = buffer.as_raw_buffer();
        if self.scratch_buffer.len() != frame_data.len() {
            self.scratch_buffer.resize(frame_data.len(), 0);
        }
        self.scratch_buffer.copy_from_slice(frame_data);

        self.current_frame_id += 1;

        {
            let mut shared = self.shared_buffer.lock().unwrap();
            std::mem::swap(&mut shared.buffer, &mut self.scratch_buffer);
            shared.frame_id = self.current_frame_id;
        }
        Ok(())
    }
}
