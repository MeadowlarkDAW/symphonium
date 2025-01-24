use std::borrow::Cow;

use symphonia::core::audio::AudioBufferRef;
use symphonia::core::audio::{AudioBuffer, Signal};
use symphonia::core::codecs::{CodecRegistry, DecoderOptions};
use symphonia::core::probe::ProbeResult;
use symphonia::core::sample::{i24, u24};

use crate::DecodedAudioF32;

use super::resource::{DecodedAudio, DecodedAudioType};
use super::{convert, LoadError};

const SHRINK_THRESHOLD: usize = 4096;

#[cfg(feature = "resampler")]
pub(crate) fn decode_resampled(
    probed: &mut ProbeResult,
    codec_registry: &CodecRegistry,
    pcm_sample_rate: u32,
    target_sample_rate: u32,
    n_channels: usize,
    resampler: &mut fixed_resample::NonRtResampler<f32>,
    max_bytes: usize,
) -> Result<DecodedAudioF32, LoadError> {
    assert_ne!(n_channels, 0);

    resampler.reset();

    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let file_frames = track.codec_params.n_frames;
    let max_frames = max_bytes / (4 * n_channels);

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

    let alloc_final_frames = resampler.out_alloc_frames(
        pcm_sample_rate,
        target_sample_rate,
        file_frames.unwrap_or(44100) as usize,
    );
    let mut final_buf: Vec<Vec<f32>> = (0..n_channels)
        .map(|_| {
            let mut m = Vec::new();
            m.reserve_exact(alloc_final_frames);
            m
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

                resampler.process(
                    tmp_conversion_buf.planes().planes(),
                    |in_buf, in_buf_frames| {
                        for (out_ch, in_ch) in final_buf.iter_mut().zip(in_buf.iter()) {
                            out_ch.extend_from_slice(&in_ch[..in_buf_frames]);
                        }
                    },
                    false,
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

    let out_frames = resampler.out_frames(pcm_sample_rate, target_sample_rate, total_in_frames);

    // Process any leftover samples in the resampler.
    while final_buf[0].len() < out_frames {
        resampler.process::<&[f32]>(
            &[],
            |in_buf, in_buf_frames| {
                for (out_ch, in_ch) in final_buf.iter_mut().zip(in_buf.iter()) {
                    out_ch.extend_from_slice(&in_ch[..in_buf_frames]);
                }
            },
            true,
        );
    }

    // Truncate any extra padded zeros off the end.
    for ch in final_buf.iter_mut() {
        ch.resize(out_frames, 0.0);

        // If the allocated capacity is significantly greater than the
        // length, shrink it to save memory.
        if ch.capacity() > ch.len() + SHRINK_THRESHOLD {
            ch.shrink_to_fit();
        }
    }

    Ok(DecodedAudioF32::new(final_buf, target_sample_rate))
}

pub(crate) fn decode_f32(
    probed: &mut ProbeResult,
    n_channels: usize,
    codec_registry: &CodecRegistry,
    sample_rate: u32,
    max_bytes: usize,
) -> Result<DecodedAudioF32, LoadError> {
    assert_ne!(n_channels, 0);

    // Get the default track in the audio stream.
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| LoadError::NoTrackFound)?;

    let file_frames = track.codec_params.n_frames;
    let max_frames = max_bytes / (4 * n_channels);

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

    let estimated_final_frames = file_frames.unwrap_or(44100) as usize;
    let mut final_buf: Vec<Vec<f32>> = (0..n_channels)
        .map(|_| {
            let mut m = Vec::new();
            m.reserve_exact(estimated_final_frames);
            m
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
    n_channels: usize,
    codec_registry: &CodecRegistry,
    sample_rate: u32,
    max_bytes: usize,
) -> Result<DecodedAudio, LoadError> {
    assert_ne!(n_channels, 0);

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
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / n_channels;
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u8_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::U8(decoded_channels));
                    break;
                }
                AudioBufferRef::U16(d) => {
                    let mut decoded_channels = Vec::<Vec<u16>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (2 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u16_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::U16(decoded_channels));
                    break;
                }
                AudioBufferRef::U24(d) => {
                    let mut decoded_channels = Vec::<Vec<[u8; 3]>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (3 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u24_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::U24(decoded_channels));
                    break;
                }
                AudioBufferRef::U32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_u32_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::U32(decoded_channels));
                    break;
                }
                AudioBufferRef::S8(d) => {
                    let mut decoded_channels = Vec::<Vec<i8>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / n_channels;
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i8_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::S8(decoded_channels));
                    break;
                }
                AudioBufferRef::S16(d) => {
                    let mut decoded_channels = Vec::<Vec<i16>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (2 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i16_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::S16(decoded_channels));
                    break;
                }
                AudioBufferRef::S24(d) => {
                    let mut decoded_channels = Vec::<Vec<[u8; 3]>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (3 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i24_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::S24(decoded_channels));
                    break;
                }
                AudioBufferRef::S32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_i32_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::S32(decoded_channels));
                    break;
                }
                AudioBufferRef::F32(d) => {
                    let mut decoded_channels = Vec::<Vec<f32>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (4 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_f32_packet(&mut decoded_channels, d, n_channels);

                    first_packet = Some(FirstPacketType::F32(decoded_channels));
                    break;
                }
                AudioBufferRef::F64(d) => {
                    let mut decoded_channels = Vec::<Vec<f64>>::new();
                    for _ in 0..n_channels {
                        decoded_channels
                            .push(Vec::with_capacity(file_frames.unwrap_or(0) as usize));
                    }

                    max_frames = max_bytes / (8 * n_channels);
                    if let Some(file_frames) = file_frames {
                        if file_frames > max_frames as u64 {
                            return Err(LoadError::FileTooLarge(max_bytes));
                        }
                    } else {
                        check_total_frames(&mut total_frames, max_frames, d.chan(0).len())?;
                    }

                    decode_f64_packet(&mut decoded_channels, d, n_channels);

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

                            decode_u8_packet(&mut decoded_channels, d, n_channels);
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

                            decode_u16_packet(&mut decoded_channels, d, n_channels);
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

                            decode_u24_packet(&mut decoded_channels, d, n_channels);
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

                            decode_u32_packet(&mut decoded_channels, d, n_channels);
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

                            decode_i8_packet(&mut decoded_channels, d, n_channels);
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

                            decode_i16_packet(&mut decoded_channels, d, n_channels);
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

                            decode_i24_packet(&mut decoded_channels, d, n_channels);
                        }
                        _ => return Err(unexpected_format("i24")),
                    },
                    Err(symphonia::core::errors::Error::DecodeError(err)) => decode_warning(err),
                    Err(e) => return Err(LoadError::ErrorWhileDecoding(e)),
                }
            }

            shrink_buffer(&mut decoded_channels);

            DecodedAudioType::S24(decoded_channels)
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

                            decode_i32_packet(&mut decoded_channels, d, n_channels);
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

                            decode_f32_packet(&mut decoded_channels, d, n_channels);
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

                            decode_f64_packet(&mut decoded_channels, d, n_channels);
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
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_u16_packet(
    decoded_channels: &mut Vec<Vec<u16>>,
    packet: Cow<AudioBuffer<u16>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_u24_packet(
    decoded_channels: &mut Vec<Vec<[u8; 3]>>,
    packet: Cow<AudioBuffer<u24>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        for s in packet.chan(i).iter() {
            decoded_channels[i].push(s.to_ne_bytes());
        }
    }
}

#[inline]
fn decode_u32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<u32>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        for s in packet.chan(i).iter() {
            let s_f32 = convert::pcm_u32_to_f32(*s);

            decoded_channels[i].push(s_f32);
        }
    }
}

#[inline]
fn decode_i8_packet(
    decoded_channels: &mut Vec<Vec<i8>>,
    packet: Cow<AudioBuffer<i8>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_i16_packet(
    decoded_channels: &mut Vec<Vec<i16>>,
    packet: Cow<AudioBuffer<i16>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_i24_packet(
    decoded_channels: &mut Vec<Vec<[u8; 3]>>,
    packet: Cow<AudioBuffer<i24>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        for s in packet.chan(i).iter() {
            decoded_channels[i].push(s.to_ne_bytes());
        }
    }
}

#[inline]
fn decode_i32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<i32>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        for s in packet.chan(i).iter() {
            let s_f32 = convert::pcm_i32_to_f32(*s);

            decoded_channels[i].push(s_f32);
        }
    }
}

#[inline]
fn decode_f32_packet(
    decoded_channels: &mut Vec<Vec<f32>>,
    packet: Cow<AudioBuffer<f32>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

#[inline]
fn decode_f64_packet(
    decoded_channels: &mut Vec<Vec<f64>>,
    packet: Cow<AudioBuffer<f64>>,
    num_channels: usize,
) {
    for i in 0..num_channels {
        decoded_channels[i].extend_from_slice(packet.chan(i));
    }
}

fn decode_warning(err: &str) {
    // Decode errors are not fatal. Print the error message and try to decode the next
    // packet as usual.
    log::warn!("Symphonia decode warning: {}", err);
}
