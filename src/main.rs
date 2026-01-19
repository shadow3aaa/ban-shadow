#[cfg(not(target_os = "windows"))]
compile_error!("Only supports Windows for now.");

mod app;
mod capture;

use clap::Parser;
use winit::event_loop::EventLoop;

use crate::app::AppHandler;

#[derive(clap::Parser)]
struct Args {}

fn main() -> anyhow::Result<()> {
    let _args = Args::try_parse()?;
    let event_loop = EventLoop::new()?;

    event_loop.run_app(&mut AppHandler::default())?;
    Ok(())
}
