use std::fs::File;
use std::path::Path;

#[cfg(feature = "resampler")]
use std::collections::HashMap;

use fixed_resample::NonRtResampler;
use symphonia::core::codecs::CodecRegistry;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::{Hint, Probe, ProbeResult};

// Re-export symphonia
pub use symphonia;

pub mod convert;
pub mod error;

#[cfg(feature = "resampler")]
pub mod resample;
#[cfg(feature = "resampler")]
pub use resample::ResampleQuality;
#[cfg(feature = "resampler")]
use resample::{ResamplerKey, ResamplerParams};

mod decode;
mod resource;

pub use resource::*;

use error::LoadError;

/// The default maximum size of an audio file in bytes.
pub static DEFAULT_MAX_BYTES: usize = 1_000_000_000;

/// Used to load audio files into RAM. This stores samples in
/// their native sample format when possible to save memory.
pub struct SymphoniumLoader {
    // Re-use resamplers to improve performance.
    #[cfg(feature = "resampler")]
    resamplers: HashMap<ResamplerKey, NonRtResampler<f32>>,

    codec_registry: &'static CodecRegistry,
    probe: &'static Probe,
}

impl SymphoniumLoader {
    /// Construct a new audio file loader.
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "resampler")]
            resamplers: HashMap::new(),
            codec_registry: symphonia::default::get_codecs(),
            probe: symphonia::default::get_probe(),
        }
    }

    /// Load an audio file from the given path into RAM.
    ///
    /// * `path` - The path to the audio file stored on disk.
    /// * `target_sample_rate` - If this is `Some`, then the file will be resampled to that
    /// sample rate. (No resampling will occur if the audio file's sample rate is already
    /// the target sample rate). If this is `None`, then the file will not be resampled
    /// and it will stay its original sample rate.
    ///     * Note that resampling will always convert the sample format to `f32`. If
    /// saving memory is a concern, then set this to `None` and resample in realtime.
    /// * `resample_quality` - The quality of the resampler to use if the `target_sample_rate`
    /// doesn't match the source sample rate.
    ///     - Has no effect if `target_sample_rate` is `None`.
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    pub fn load<P: AsRef<Path>>(
        &mut self,
        path: P,
        #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
        #[cfg(feature = "resampler")] resample_quality: ResampleQuality,
        max_bytes: Option<usize>,
    ) -> Result<DecodedAudio, LoadError> {
        let source = load_file(path, self.probe)?;

        decode(
            source,
            self.codec_registry,
            max_bytes,
            #[cfg(feature = "resampler")]
            target_sample_rate,
            #[cfg(feature = "resampler")]
            |params| {
                self::resample::get_resampler(
                    &mut self.resamplers,
                    resample_quality,
                    params.source_sample_rate,
                    params.target_sample_rate,
                    params.num_channels,
                )
            },
        )
    }

    /// Load an audio source into RAM.
    ///
    /// * `source` - The audio source which implements the [`MediaSource`] trait.
    /// * `hint` - An optional hint to help the format registry guess what format reader is
    /// appropriate.
    /// * `target_sample_rate` - If this is `Some`, then the file will be resampled to that
    /// sample rate. (No resampling will occur if the audio file's sample rate is already
    /// the target sample rate). If this is `None`, then the file will not be resampled
    /// and it will stay its original sample rate.
    ///     * Note that resampling will always convert the sample format to `f32`. If
    /// saving memory is a concern, then set this to `None` and resample in realtime.
    /// * `resample_quality` - The quality of the resampler to use if the `target_sample_rate`
    /// doesn't match the source sample rate.
    ///     - Has no effect if `target_sample_rate` is `None`.
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    pub fn load_from_source(
        &mut self,
        source: Box<dyn MediaSource>,
        hint: Option<Hint>,
        #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
        #[cfg(feature = "resampler")] resample_quality: ResampleQuality,
        max_bytes: Option<usize>,
    ) -> Result<DecodedAudio, LoadError> {
        let source = load_audio_source(source, hint, self.probe)?;

        decode(
            source,
            self.codec_registry,
            max_bytes,
            #[cfg(feature = "resampler")]
            target_sample_rate,
            #[cfg(feature = "resampler")]
            |params| {
                self::resample::get_resampler(
                    &mut self.resamplers,
                    resample_quality,
                    params.source_sample_rate,
                    params.target_sample_rate,
                    params.num_channels,
                )
            },
        )
    }

    /// Load an audio file from the given path into RAM using a custom resampler.
    ///
    /// * `path` - The path to the audio file stored on disk.
    /// * `target_sample_rate` - The target sample rate. (No resampling will occur if the audio
    /// file's sample rate is already the target sample rate).
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    /// * `get_resampler` - Get the custom sampler with the desired parameters.
    #[cfg(feature = "resampler")]
    pub fn load_with_resampler<'a, P: AsRef<Path>>(
        &mut self,
        path: P,
        target_sample_rate: u32,
        max_bytes: Option<usize>,
        get_resampler: impl FnOnce(ResamplerParams) -> &'a mut NonRtResampler<f32>,
    ) -> Result<DecodedAudio, LoadError> {
        let source = load_file(path, self.probe)?;

        decode(
            source,
            self.codec_registry,
            max_bytes,
            Some(target_sample_rate),
            get_resampler,
        )
    }

    /// Load an audio source into RAM using a custom resampler.
    ///
    /// * `source` - The audio source which implements the [`MediaSource`] trait.
    /// * `hint` - An optional hint to help the format registry guess what format reader is
    /// appropriate.
    /// * `target_sample_rate` - The target sample rate. (No resampling will occur if the audio
    /// file's sample rate is already the target sample rate).
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    /// * `get_resampler` - Get the custom sampler with the desired parameters.
    #[cfg(feature = "resampler")]
    pub fn load_from_source_with_resampler<'a>(
        &mut self,
        source: Box<dyn MediaSource>,
        hint: Option<Hint>,
        target_sample_rate: u32,
        max_bytes: Option<usize>,
        get_resampler: impl FnOnce(ResamplerParams) -> &'a mut NonRtResampler<f32>,
    ) -> Result<DecodedAudio, LoadError> {
        let source = load_audio_source(source, hint, self.probe)?;

        decode(
            source,
            self.codec_registry,
            max_bytes,
            Some(target_sample_rate),
            get_resampler,
        )
    }

    /// Load an audio file from the given path into RAM and convert to an f32 sample format.
    ///
    /// * `path` - The path to the audio file stored on disk.
    /// * `target_sample_rate` - If this is `Some`, then the file will be resampled to that
    /// sample rate. (No resampling will occur if the audio file's sample rate is already
    /// the target sample rate). If this is `None`, then the file will not be resampled
    /// and it will stay its original sample rate.
    ///     * Note that resampling will always convert the sample format to `f32`. If
    /// saving memory is a concern, then set this to `None` and resample in realtime.
    /// * `resample_quality` - The quality of the resampler to use if the `target_sample_rate`
    /// doesn't match the source sample rate.
    ///     - Has no effect if `target_sample_rate` is `None`.
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    pub fn load_f32<P: AsRef<Path>>(
        &mut self,
        path: P,
        #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
        #[cfg(feature = "resampler")] resample_quality: ResampleQuality,
        max_bytes: Option<usize>,
    ) -> Result<DecodedAudioF32, LoadError> {
        let source = load_file(path, self.probe)?;

        decode_f32(
            source,
            self.codec_registry,
            max_bytes,
            #[cfg(feature = "resampler")]
            target_sample_rate,
            #[cfg(feature = "resampler")]
            |params| {
                self::resample::get_resampler(
                    &mut self.resamplers,
                    resample_quality,
                    params.source_sample_rate,
                    params.target_sample_rate,
                    params.num_channels,
                )
            },
        )
    }

    /// Load an audio source into RAM and convert to an f32 sample format.
    ///
    /// * `source` - The audio source which implements the [`MediaSource`] trait.
    /// * `hint` - An optional hint to help the format registry guess what format reader is
    /// appropriate.
    /// * `target_sample_rate` - If this is `Some`, then the file will be resampled to that
    /// sample rate. (No resampling will occur if the audio file's sample rate is already
    /// the target sample rate). If this is `None`, then the file will not be resampled
    /// and it will stay its original sample rate.
    ///     * Note that resampling will always convert the sample format to `f32`. If
    /// saving memory is a concern, then set this to `None` and resample in realtime.
    /// * `resample_quality` - The quality of the resampler to use if the `target_sample_rate`
    /// doesn't match the source sample rate.
    ///     - Has no effect if `target_sample_rate` is `None`.
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    pub fn load_f32_from_source(
        &mut self,
        source: Box<dyn MediaSource>,
        hint: Option<Hint>,
        #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
        #[cfg(feature = "resampler")] resample_quality: ResampleQuality,
        max_bytes: Option<usize>,
    ) -> Result<DecodedAudioF32, LoadError> {
        let source = load_audio_source(source, hint, self.probe)?;

        decode_f32(
            source,
            self.codec_registry,
            max_bytes,
            #[cfg(feature = "resampler")]
            target_sample_rate,
            #[cfg(feature = "resampler")]
            |params| {
                self::resample::get_resampler(
                    &mut self.resamplers,
                    resample_quality,
                    params.source_sample_rate,
                    params.target_sample_rate,
                    params.num_channels,
                )
            },
        )
    }

    /// Load an audio source into RAM using a custom resampler and convert to an f32 sample
    /// format.
    ///
    /// * `path` - The path to the audio file stored on disk.
    /// * `target_sample_rate` - The target sample rate. (No resampling will occur if the audio
    /// file's sample rate is already the target sample rate).
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    /// * `get_resampler` - Get the custom sampler with the desired parameters.
    #[cfg(feature = "resampler")]
    pub fn load_f32_with_resampler<'a, P: AsRef<Path>>(
        &mut self,
        path: P,
        target_sample_rate: u32,
        max_bytes: Option<usize>,
        get_resampler: impl FnOnce(ResamplerParams) -> &'a mut NonRtResampler<f32>,
    ) -> Result<DecodedAudioF32, LoadError> {
        let source = load_file(path, self.probe)?;

        decode_f32(
            source,
            self.codec_registry,
            max_bytes,
            Some(target_sample_rate),
            get_resampler,
        )
    }

    /// Load an audio source into RAM using a custom resampler and convert to an f32 sample
    /// format.
    ///
    /// * `source` - The audio source which implements the [`MediaSource`] trait.
    /// * `hint` - An optional hint to help the format registry guess what format reader is
    /// appropriate.
    /// * `target_sample_rate` - The target sample rate. (No resampling will occur if the audio
    /// file's sample rate is already the target sample rate).
    /// * `max_bytes` - The maximum size in bytes that the resulting `DecodedAudio`
    /// resource can  be in RAM. If the resulting resource is larger than this, then an error
    /// will be returned instead. This is useful to avoid locking up or crashing the system
    /// if the use tries to load a really large audio file.
    ///     * If this is `None`, then default of `1_000_000_000` (1GB) will be used.
    /// * `get_resampler` - Get the custom sampler with the desired parameters.
    #[cfg(feature = "resampler")]
    pub fn load_f32_from_source_with_resampler<'a>(
        &mut self,
        source: Box<dyn MediaSource>,
        hint: Option<Hint>,
        target_sample_rate: u32,
        max_bytes: Option<usize>,
        get_resampler: impl FnOnce(ResamplerParams) -> &'a mut NonRtResampler<f32>,
    ) -> Result<DecodedAudioF32, LoadError> {
        let source = load_audio_source(source, hint, self.probe)?;

        decode_f32(
            source,
            self.codec_registry,
            max_bytes,
            Some(target_sample_rate),
            get_resampler,
        )
    }
}

struct LoadedAudioSource {
    probed: ProbeResult,
    sample_rate: u32,
    n_channels: usize,
}

fn load_file<P: AsRef<Path>>(
    path: P,
    probe: &'static Probe,
) -> Result<LoadedAudioSource, LoadError> {
    let path: &Path = path.as_ref();

    // Try to open the file.
    let file = File::open(path)?;

    // Create a hint to help the format registry guess what format reader is appropriate.
    let mut hint = Hint::new();

    // Provide the file extension as a hint.
    if let Some(extension) = path.extension() {
        if let Some(extension_str) = extension.to_str() {
            hint.with_extension(extension_str);
        }
    }

    load_audio_source(Box::new(file), Some(hint), probe)
}

fn load_audio_source(
    source: Box<dyn MediaSource>,
    hint: Option<Hint>,
    probe: &'static Probe,
) -> Result<LoadedAudioSource, LoadError> {
    // Create the media source stream.
    let mss = MediaSourceStream::new(source, Default::default());

    // Use the default options for format reader, metadata reader, and decoder.
    let format_opts: FormatOptions = Default::default();
    let metadata_opts: MetadataOptions = Default::default();

    let hint = hint.unwrap_or_default();

    // Probe the media source stream for metadata and get the format reader.
    let probed = probe
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| LoadError::UnkownFormat(e))?;

    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let sample_rate = track.codec_params.sample_rate.unwrap_or_else(|| {
        log::warn!("Could not find sample rate of PCM resource. Assuming a sample rate of 44100");
        44100
    });

    let n_channels = track
        .codec_params
        .channels
        .ok_or_else(|| LoadError::NoChannelsFound)?
        .count();

    if n_channels == 0 {
        return Err(LoadError::NoChannelsFound);
    }

    Ok(LoadedAudioSource {
        probed,
        sample_rate,
        n_channels,
    })
}

fn decode<'a>(
    mut source: LoadedAudioSource,
    codec_registry: &'static CodecRegistry,
    max_bytes: Option<usize>,
    #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
    #[cfg(feature = "resampler")] get_resampler: impl FnOnce(
        ResamplerParams,
    ) -> &'a mut NonRtResampler<f32>,
) -> Result<DecodedAudio, LoadError> {
    #[cfg(feature = "resampler")]
    if let Some(target_sample_rate) = target_sample_rate {
        if source.sample_rate != target_sample_rate {
            // Resampling is needed.
            return resample(
                source,
                codec_registry,
                max_bytes,
                target_sample_rate,
                get_resampler,
            )
            .map(|pcm| pcm.into());
        }
    }

    let pcm = decode::decode_native_bitdepth(
        &mut source.probed,
        source.n_channels,
        codec_registry,
        source.sample_rate,
        max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
    )?;

    Ok(pcm)
}

fn decode_f32<'a>(
    mut source: LoadedAudioSource,
    codec_registry: &'static CodecRegistry,
    max_bytes: Option<usize>,
    #[cfg(feature = "resampler")] target_sample_rate: Option<u32>,
    #[cfg(feature = "resampler")] get_resampler: impl FnOnce(
        ResamplerParams,
    ) -> &'a mut NonRtResampler<f32>,
) -> Result<DecodedAudioF32, LoadError> {
    #[cfg(feature = "resampler")]
    if let Some(target_sample_rate) = target_sample_rate {
        if source.sample_rate != target_sample_rate {
            // Resampling is needed.
            return resample(
                source,
                codec_registry,
                max_bytes,
                target_sample_rate,
                get_resampler,
            );
        }
    }

    let pcm = decode::decode_f32(
        &mut source.probed,
        source.n_channels,
        codec_registry,
        source.sample_rate,
        max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
    )?;

    Ok(pcm)
}

#[cfg(feature = "resampler")]
fn resample<'a>(
    mut source: LoadedAudioSource,
    codec_registry: &'static CodecRegistry,
    max_bytes: Option<usize>,
    target_sample_rate: u32,
    get_resampler: impl FnOnce(ResamplerParams) -> &'a mut NonRtResampler<f32>,
) -> Result<DecodedAudioF32, LoadError> {
    let resampler = get_resampler(ResamplerParams {
        num_channels: source.n_channels,
        source_sample_rate: source.sample_rate,
        target_sample_rate,
    });

    if resampler.num_channels().get() != source.n_channels {
        return Err(LoadError::InvalidResampler {
            needed_channels: source.n_channels,
            got_channels: resampler.num_channels().get(),
        });
    }

    let pcm = decode::decode_resampled(
        &mut source.probed,
        codec_registry,
        source.sample_rate,
        target_sample_rate,
        source.n_channels,
        resampler,
        max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
    )?;

    return Ok(pcm);
}
