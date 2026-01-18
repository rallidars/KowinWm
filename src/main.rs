mod handlers;
mod state;
mod udev;
mod utils;
use smithay::utils::SerialCounter;

use crate::utils::logs::init_logs;

pub static SERIAL_COUNTER: SerialCounter = SerialCounter::new();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logs();

    udev::init_udev();

    Ok(())
}
