use std::borrow::Cow;
use std::num::NonZeroUsize;

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::{AudioBuffer, Signal};
use symphonia::core::codecs::{CodecRegistry, DecoderOptions};
use symphonia::core::conv::FromSample;
use symphonia::core::probe::ProbeResult;
use symphonia::core::sample::{i24, u24};

use crate::DecodedAudioF32;

use super::resource::{DecodedAudio, DecodedAudioType};
use super::LoadError;

const SHRINK_THRESHOLD: usize = 4096;

#[cfg(feature = "resampler")]
pub(crate) fn decode_resampled(
    probed: &mut ProbeResult,
    codec_registry: &CodecRegistry,
    target_sample_rate: u32,
    num_channels: NonZeroUsize,
    resampler: &mut fixed_resample::FixedResampler<f32, { crate::MAX_CHANNELS }>,
    max_bytes: usize,
) -> Result<DecodedAudioF32, LoadError> {
    use fixed_resample::LastPacketInfo;

    resampler.reset();

    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let file_frames = track.codec_params.n_frames;
    let max_frames = max_bytes / (4 * num_channels.get());

    if let Some(frames) = file_frames {
        if frames > max_frames as u64 {
            return Err(LoadError::FileTooLarge(max_bytes));
        }
    }

    let decode_opts: DecoderOptions = Default::default();

    // Create a decoder for the track.
    let mut decoder = codec_registry
        .make(&track.codec_params, &decode_opts)
        .map_err(|e| LoadError::CouldNotCreateDecoder(e))?;

    let mut tmp_conversion_buf: Option<AudioBuffer<f32>> = None;

    let final_output_frames = file_frames.map(|f| resampler.out_alloc_frames(f) as usize);

    let alloc_frames = file_frames.unwrap_or(32768) as usize;
    let mut final_buf: Vec<Vec<f32>> = (0..num_channels.get())
        .map(|_| {
            let mut v = Vec::new();
            v.reserve_exact(alloc_frames);
            v
        })
        .collect();

    let track_id = track.id;

    let mut total_in_frames: usize = 0;

    while let Ok(packet) = probed.format.next_packet() {
        // If the packet does not belong to the selected track, skip over it.
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                // If this is the first decoded packet, allocate the temporary conversion
                // buffer with the required capacity.
                if tmp_conversion_buf.is_none() {
                    let spec = *(decoded.spec());
                    let duration = decoded.capacity();

                    tmp_conversion_buf = Some(AudioBuffer::new(duration as u64, spec));
                }
                let tmp_conversion_buf = tmp_conversion_buf.as_mut().unwrap();
                if tmp_conversion_buf.capacity() < decoded.capacity() {
                    let spec = *(decoded.spec());
                    let duration = decoded.capacity();

                    *tmp_conversion_buf = AudioBuffer::new(duration as u64, spec);
                }

                decoded.convert(tmp_conversion_buf);

                let decoded_frames = tmp_conversion_buf.frames();
                total_in_frames += decoded_frames;

                let last_packet = if let Some(file_frames) = file_frames {
                    if total_in_frames as u64 >= file_frames {
                        let desired_output_frames =
                            if final_buf[0].len() <= final_output_frames.unwrap() {
                                Some((final_output_frames.unwrap() - final_buf[0].len()) as u64)
                            } else {
                                None
                            };

                        Some(LastPacketInfo {
                            desired_output_frames,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                resampler.process(
                    tmp_conversion_buf.planes().planes(),
                    0..decoded_frames,
                    |in_buf| {
                        for (out_ch, in_ch) in final_buf.iter_mut().zip(in_buf.iter()) {
                            out_ch.extend_from_slice(in_ch);
                        }
                    },
                    last_packet,
                    true, // trim_delay
                );

                if file_frames.is_none() {
                    // Protect against really large files causing out of memory errors.
                    if final_buf[0].len() > max_frames {
                        return Err(LoadError::FileTooLarge(max_bytes));
                    }
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
            Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
        }
    }

    // Process any leftover samples in the resampler.
    if file_frames.is_none() {
        let final_output_frames = resampler.out_alloc_frames(total_in_frames as u64) as usize;

        if final_buf[0].len() < final_output_frames {
            let desired_output_frames = final_output_frames - final_buf[0].len();

            resampler.process::<&[f32]>(
                &[],
                0..0,
                |in_buf| {
                    for (out_ch, in_ch) in final_buf.iter_mut().zip(in_buf.iter()) {
                        out_ch.extend_from_slice(in_ch);
                    }
                },
                Some(LastPacketInfo {
                    desired_output_frames: Some(desired_output_frames as u64),
                }),
                true, // trim_delay
            );
        } else {
            for ch in final_buf.iter_mut() {
                ch.resize(final_output_frames, 0.0);
            }
        }

        shrink_buffer(&mut final_buf);
    } else {
        // Sanity check to make sure the resampler output the correct
        // number of frames.
        assert_eq!(final_buf[0].len(), final_output_frames.unwrap());
    }

    Ok(DecodedAudioF32::new(final_buf, target_sample_rate))
}

pub(crate) fn decode_f32(
    probed: &mut ProbeResult,
    num_channels: NonZeroUsize,
    codec_registry: &CodecRegistry,
    sample_rate: u32,
    max_bytes: usize,
) -> Result<DecodedAudioF32, LoadError> {
    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let file_frames = track.codec_params.n_frames;
    let max_frames = max_bytes / (4 * num_channels.get());

    if let Some(frames) = file_frames {
        if frames > max_frames as u64 {
            return Err(LoadError::FileTooLarge(max_bytes));
        }
    }

    let decode_opts: DecoderOptions = Default::default();

    // Create a decoder for the track.
    let mut decoder = codec_registry
        .make(&track.codec_params, &decode_opts)
        .map_err(|e| LoadError::CouldNotCreateDecoder(e))?;

    let mut tmp_conversion_buf: Option<AudioBuffer<f32>> = None;

    let estimated_final_frames = file_frames.unwrap_or(32768) as usize;
    let mut final_buf: Vec<Vec<f32>> = (0..num_channels.get())
        .map(|_| {
            let mut v = Vec::new();
            v.reserve_exact(estimated_final_frames);
            v
        })
        .collect();

    let track_id = track.id;

    while let Ok(packet) = probed.format.next_packet() {
        // If the packet does not belong to the selected track, skip over it.
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                // If this is the first decoded packet, allocate the temporary conversion
                // buffer with the required capacity.
                if tmp_conversion_buf.is_none() {
                    let spec = *(decoded.spec());
                    let duration = decoded.capacity();

                    tmp_conversion_buf = Some(AudioBuffer::new(duration as u64, spec));
                }
                let tmp_conversion_buf = tmp_conversion_buf.as_mut().unwrap();
                if tmp_conversion_buf.capacity() < decoded.capacity() {
                    let spec = *(decoded.spec());
                    let duration = decoded.capacity();

                    *tmp_conversion_buf = AudioBuffer::new(duration as u64, spec);
                }

                decoded.convert(tmp_conversion_buf);

                let tmp_conversion_planes = tmp_conversion_buf.planes();
                let converted_planes = tmp_conversion_planes.planes();

                for (final_ch, decoded_ch) in final_buf.iter_mut().zip(converted_planes) {
                    final_ch.extend_from_slice(&decoded_ch);
                }

                if file_frames.is_none() {
                    // Protect against really large files causing out of memory errors.
                    if final_buf[0].len() > max_frames {
                        return Err(LoadError::FileTooLarge(max_bytes));
                    }
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
            Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
        }
    }

    shrink_buffer(&mut final_buf);

    Ok(DecodedAudioF32::new(final_buf, sample_rate))
}

pub(crate) fn decode_native_bitdepth(
    probed: &mut ProbeResult,
    num_channels: NonZeroUsize,
    codec_registry: &CodecRegistry,
    sample_rate: u32,
    max_bytes: usize,
) -> Result<DecodedAudio, LoadError> {
    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let file_frames = track.codec_params.n_frames;

    let decode_opts: DecoderOptions = Default::default();

    // Create a decoder for the track.
    let mut decoder = codec_registry
        .make(&track.codec_params, &decode_opts)
        .map_err(|e| LoadError::CouldNotCreateDecoder(e))?;

    let mut max_frames = 0;
    let mut total_frames = 0;

    enum FirstPacketType {
        U8(Vec<Vec<u8>>),
        U16(Vec<Vec<u16>>),
        U24(Vec<Vec<[u8; 3]>>),
        U32(Vec<Vec<f32>>),
        S8(Vec<Vec<i8>>),
        S16(Vec<Vec<i16>>),
        S24(Vec<Vec<[u8; 3]>>),
        S32(Vec<Vec<f32>>),
        F32(Vec<Vec<f32>>),
        F64(Vec<Vec<f64>>),
    }

    let track_id = track.id;

    let check_total_frames =
        |total_frames: &mut usize, max_frames: usize, packet_len: usize| -> Result<(), LoadError> {
            *total_frames += packet_len;
            if *total_frames > max_frames {
                Err(LoadError::FileTooLarge(max_bytes))
            } else {
                Ok(())
            }
        };

    // Decode the first packet to get the sample format.
    let mut first_packet = None;
    while let Ok(packet) = probed.format.next_packet() {
        // If the packet does not belong to the selected track, skip over it.
        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => match decoded {
                AudioBufferRef::U8(d) => {
                    let mut decoded_channels = Vec::<Vec<u8>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / num_channels;
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u8_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::U8(decoded_channels));
                    break;
                }
                AudioBufferRef::U16(d) => {
                    let mut decoded_channels = Vec::<Vec<u16>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (2 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u16_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::U16(decoded_channels));
                    break;
                }
                AudioBufferRef::U24(d) => {
                    let mut decoded_channels = Vec::<Vec<[u8; 3]>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (3 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u24_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::U24(decoded_channels));
                    break;
                }
                AudioBufferRef::U32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u32_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::U32(decoded_channels));
                    break;
                }
                AudioBufferRef::S8(d) => {
                    let mut decoded_channels = Vec::<Vec<i8>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / num_channels;
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i8_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::S8(decoded_channels));
                    break;
                }
                AudioBufferRef::S16(d) => {
                    let mut decoded_channels = Vec::<Vec<i16>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (2 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i16_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::S16(decoded_channels));
                    break;
                }
                AudioBufferRef::S24(d) => {
                    let mut decoded_channels = Vec::<Vec<[u8; 3]>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (3 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i24_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::S24(decoded_channels));
                    break;
                }
                AudioBufferRef::S32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i32_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::S32(decoded_channels));
                    break;
                }
                AudioBufferRef::F32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_f32_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::F32(decoded_channels));
                    break;
                }
                AudioBufferRef::F64(d) => {
                    let mut decoded_channels = Vec::<Vec<f64>>::new();
                    for _ in 0..num_channels.get() {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (8 * num_channels.get());
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_f64_packet(&mut decoded_channels, d, num_channels);

                    first_packet = Some(FirstPacketType::F64(decoded_channels));
                    break;
                }
            },
            Err(symphonia::core::errors::Error::DecodeError(err)) => {
                decode_warning(err);
            }
            Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
        };
    }

    if first_packet.is_none() {
        return Err(LoadError::UnexpectedErrorWhileDecoding(
            "no packet was found".into(),
        ));
    }

    let unexpected_format = |expected: &str| -> LoadError {
        LoadError::UnexpectedErrorWhileDecoding(
            format!(
                "Symphonia returned a packet that was not the expected format of {}",
                expected
            )
            .into(),
        )
    };

    let pcm_type = match first_packet.take().unwrap() {
        FirstPacketType::U8(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::U8(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_u8_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("u8")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::U8(decoded_channels)
        }
        FirstPacketType::U16(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::U16(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_u16_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("u16")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::U16(decoded_channels)
        }
        FirstPacketType::U24(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::U24(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_u24_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("u24")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::U24(decoded_channels)
        }
        FirstPacketType::U32(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::U32(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_u32_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("u32")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::F32(decoded_channels)
        }
        FirstPacketType::S8(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::S8(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_i8_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("i8")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::S8(decoded_channels)
        }
        FirstPacketType::S16(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::S16(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_i16_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("i16")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::S16(decoded_channels)
        }
        FirstPacketType::S24(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::S24(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_i24_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("i24")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::U24(decoded_channels)
        }
        FirstPacketType::S32(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::S32(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_i32_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("i32")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::F32(decoded_channels)
        }
        FirstPacketType::F32(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::F32(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_f32_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("f32")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::F32(decoded_channels)
        }
        FirstPacketType::F64(mut decoded_channels) => {
            while let Ok(packet) = probed.format.next_packet() {
                // If the packet does not belong to the selected track, skip over it.
                if packet.track_id() != track_id {
                    continue;
                }

                match decoder.decode(&packet) {
                    Ok(decoded) => match decoded {
                        AudioBufferRef::F64(d) => {
                            if file_frames.is_none() {
                                check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                            }

                            decode_f64_packet(&mut decoded_channels, d, num_channels);
                        }
                        _ => return Err(unexpected_format("f64")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::F64(decoded_channels)
        }
    };

    Ok(DecodedAudio::new(pcm_type, sample_rate))
}

fn shrink_buffer<T>(channels: &mut [Vec<T>]) {
    for ch in channels.iter_mut() {
        // If the allocated capacity is significantly greater than the
        // length, shrink it to save memory.
        if ch.capacity() > ch.len() + SHRINK_THRESHOLD {
            ch.shrink_to_fit();
        }
    }
}

#[inline]
fn decode_u8_packet(
    decoded_channels: &mut Vec<Vec<u8>>,
    packet: Cow<AudioBuffer<u8>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_u16_packet(
    decoded_channels: &mut Vec<Vec<u16>>,
    packet: Cow<AudioBuffer<u16>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_u24_packet(
    decoded_channels: &mut Vec<Vec<[u8; 3]>>,
    packet: Cow<AudioBuffer<u24>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        for s in packet.chan(i).iter() {
            decoded_channels[i].push(s.to_ne_bytes());
        }
    }
}

#[inline]
fn decode_u32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<u32>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        for s in packet.chan(i).iter() {
            decoded_channels[i].push(FromSample::from_sample(*s));
        }
    }
}

#[inline]
fn decode_i8_packet(
    decoded_channels: &mut Vec<Vec<i8>>,
    packet: Cow<AudioBuffer<i8>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_i16_packet(
    decoded_channels: &mut Vec<Vec<i16>>,
    packet: Cow<AudioBuffer<i16>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_i24_packet(
    decoded_channels: &mut Vec<Vec<[u8; 3]>>,
    packet: Cow<AudioBuffer<i24>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        for s in packet.chan(i).iter() {
            // Converting a two's compiliment number from 32 bit to 24 bit and vice-versa
            // is tricky and less performant. So instead just convert the sample to `u24`
            // format.
            decoded_channels[i].push(u24::from_sample(*s).to_ne_bytes());
        }
    }
}

#[inline]
fn decode_i32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<i32>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        for s in packet.chan(i).iter() {
            decoded_channels[i].push(FromSample::from_sample(*s));
        }
    }
}

#[inline]
fn decode_f32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<f32>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_f64_packet(
    decoded_channels: &mut Vec<Vec<f64>>,
    packet: Cow<AudioBuffer<f64>>,
    num_channels: NonZeroUsize,
) {
    for i in 0..num_channels.get() {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

fn decode_warning(err: &str) {
    // Decode errors are not fatal. Print the error message and try to decode the next
    // packet as usual.
    log::warn!("Symphonia decode warning: {}", err);
}
