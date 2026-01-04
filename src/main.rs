mod backend;
mod handlers;
mod state;
mod utils;
use smithay::utils::SerialCounter;

use crate::utils::logs::init_logs;

pub static SERIAL_COUNTER: SerialCounter = SerialCounter::new();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logs();

    backend::udev::init_udev();

    Ok(())
}
