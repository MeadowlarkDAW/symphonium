use std::{collections::HashMap, fmt::Debug};

pub use fixed_resample;

use fixed_resample::NonRtResampler;
pub use fixed_resample::ResampleQuality;

/// The parameters to get a custom resampler
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResamplerParams {
    pub num_channels: usize,
    pub source_sample_rate: u32,
    pub target_sample_rate: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResamplerKey {
    pcm_sr: u32,
    target_sr: u32,
    channels: u16,
    quality: u16,
}

pub(crate) fn get_resampler<'a>(
    resamplers: &'a mut HashMap<ResamplerKey, NonRtResampler<f32>>,
    resample_quality: ResampleQuality,
    pcm_sr: u32,
    target_sr: u32,
    n_channels: usize,
) -> &'a mut NonRtResampler<f32> {
    let key = ResamplerKey {
        pcm_sr,
        target_sr,
        channels: n_channels as u16,
        quality: match resample_quality {
            ResampleQuality::Low => 0,
            _ => 1,
        },
    };

    resamplers
        .entry(key)
        .or_insert_with(|| NonRtResampler::new(pcm_sr, target_sr, n_channels, resample_quality))
}
