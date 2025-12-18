mod action;
mod backend;
mod handlers;
mod input;
mod render;
mod shaders;
mod state;
mod workspaces;
use smithay::utils::SerialCounter;

pub static SERIAL_COUNTER: SerialCounter = SerialCounter::new();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    backend::udev::init_udev();

    Ok(())
}
