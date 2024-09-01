use super::DeviceDescription;

pub struct LoungeTempMonitor;

impl DeviceDescription for LoungeTempMonitor {
    fn name() -> &'static str {
        "Lounge Mi Temperature and Humidity Monitor 2"
    }
}
