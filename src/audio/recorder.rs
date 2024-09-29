use anyhow::anyhow;
use cpal::{traits::DeviceTrait, Device, SampleFormat, SupportedStreamConfig};
use log::info;

use crate::config;

/// Only signed integer is supported.
const FLAC_MAX_SAMPLE_FORMAT: SampleFormat = SampleFormat::I32;

pub struct FlacRecorder {
    device: Device,
    stream_config: SupportedStreamConfig,
}

impl FlacRecorder {
    pub fn new(config: config::FlacRecorder, device: Device) -> anyhow::Result<Self> {
        let mut supported_configs: Vec<_> = device
            .supported_input_configs()?
            .filter(|stream_config| {
                let sample_format = stream_config.sample_format();
                sample_format.is_int()
                    && sample_format.sample_size() <= FLAC_MAX_SAMPLE_FORMAT.sample_size()
                    && stream_config.channels() == config.channels
            })
            .flat_map(|stream_config| stream_config.try_with_sample_rate(config.sample_rate))
            .collect();

        // Order from HIGHEST TO LOWEST priority.
        supported_configs.sort_by(|lhs, rhs| {
            let sample_size = |config: &SupportedStreamConfig| config.sample_format().sample_size();
            sample_size(rhs).cmp(&sample_size(lhs))
        });

        // Select the best option.
        if let Some(stream_config) = supported_configs.into_iter().next() {
            info!(
                "Input configuration selected: {} channel(s), sample rate {} ({})",
                stream_config.channels(),
                stream_config.sample_rate().0,
                stream_config.sample_format(),
            );
            Ok(Self {
                device,
                stream_config,
            })
        } else {
            Err(anyhow!("no supported input configurations"))
        }
    }
}
