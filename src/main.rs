mod handlers;
mod state;
mod udev;
mod utils;
use smithay::utils::SerialCounter;

use crate::utils::logs::init_logs;

pub static SERIAL_COUNTER: SerialCounter = SerialCounter::new();
pub static FALLBACK_CURSOR_DATA: &[u8] = include_bytes!("../resources/cursor.rgba");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logs();

    udev::init_udev();

    Ok(())
}
