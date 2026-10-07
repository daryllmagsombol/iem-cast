//! Device enumeration (cpal 0.18.2).
//!
//! Only confirmed cpal calls are used: `device.id()`, `device.description()`,
//! `device.supported_input_configs()`, `device.default_input_config()`,
//! `config.sample_rate()` (a `u32` alias), and `supported.try_with_sample_rate(48_000)`.
//! There is no `SampleRate(48_000)` constructor in this version.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::contract::{DeviceInfo, SampleFormat};
use crate::ids::MAX_SOURCES;

/// Enumerate input devices with their actual supported capabilities.
pub fn enumerate_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for device in devices {
        let Ok(device_id) = device.id() else {
            continue;
        };
        let id_string = device_id.to_string();
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .or_else(|_| device.id().map(|d| d.to_string()))
            .unwrap_or_else(|_| id_string.clone());
        let is_default = default_id.as_ref() == Some(&device_id);

        let mut sample_formats = Vec::new();
        let mut input_channels: u16 = 0;
        let mut buffer_min_frames: Option<u32> = None;
        let mut buffer_max_frames: Option<u32> = None;
        let mut supports_48k = false;
        let mut any = false;

        if let Ok(configs) = device.supported_input_configs() {
            for range in configs {
                any = true;
                let channels = range.channels();
                if channels >= input_channels {
                    input_channels = channels;
                }
                let format = summarize_format(range.sample_format());
                if !sample_formats.contains(&format) {
                    sample_formats.push(format);
                }
                if range.contains_rate(48_000) {
                    supports_48k = true;
                }
                if let cpal::SupportedBufferSize::Range { min, max } = range.buffer_size() {
                    buffer_min_frames = Some(buffer_min_frames.map_or(*min, |m: u32| m.min(*min)));
                    buffer_max_frames = Some(buffer_max_frames.map_or(*max, |m: u32| m.max(*max)));
                }
            }
        }

        // Never fabricate a [48000] whitelist; unknown/empty is `None`.
        let sample_rate_hz = if any && supports_48k { Some(48_000) } else { None };

        // Devices reporting more than the native maximum are rejected, not trimmed.
        if input_channels as usize > MAX_SOURCES {
            continue;
        }

        out.push(DeviceInfo {
            device_id: id_string,
            name,
            is_default,
            input_channels,
            sample_formats,
            sample_rate_hz,
            buffer_min_frames,
            buffer_max_frames,
        });
    }
    out
}

/// Map a cpal sample format onto the contract's small wire enum.
fn summarize_format(format: cpal::SampleFormat) -> SampleFormat {
    use cpal::SampleFormat as Cpal;
    match format {
        Cpal::F32 | Cpal::F64 => SampleFormat::F32,
        Cpal::I8 | Cpal::I16 | Cpal::I24 | Cpal::I32 | Cpal::I64 => SampleFormat::I16,
        Cpal::U8 | Cpal::U16 | Cpal::U24 | Cpal::U32 | Cpal::U64 => SampleFormat::U16,
        // cpal's `SampleFormat` is `#[non_exhaustive]`; brand-new formats default to f32 decode.
        _ => SampleFormat::F32,
    }
}

/// The POC whitelist of verified capture rates.
pub const SUPPORTED_RATES: [u32; 1] = [48_000];

/// Validate that `rate` is a POC-supported capture rate.
pub fn is_supported_rate(rate: u32) -> bool {
    SUPPORTED_RATES.contains(&rate)
}
