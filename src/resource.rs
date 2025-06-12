use symphonia::core::{conv::FromSample, sample::u24};

/// A resource of raw f32 audio samples stored in deinterleaved format.
///
/// This struct stores samples
/// in their native sample format when possible to save memory.
pub struct DecodedAudioF32 {
    pub data: Vec<Vec<f32>>,
    pub sample_rate: u32,
}

impl DecodedAudioF32 {
    pub fn new(data: Vec<Vec<f32>>, sample_rate: u32) -> Self {
        let frames = data[0].len();

        for ch in data.iter().skip(1) {
            assert_eq!(ch.len(), frames);
        }

        Self { data, sample_rate }
    }

    /// The number of channels in this resource.
    pub fn channels(&self) -> usize {
        self.data.len()
    }

    /// The length of this resource in frames (length of a single channel in
    /// samples).
    pub fn frames(&self) -> usize {
        self.data[0].len()
    }
}

impl Into<DecodedAudio> for DecodedAudioF32 {
    fn into(self) -> DecodedAudio {
        let channels = self.channels();
        let frames = self.frames();

        DecodedAudio {
            resource_type: DecodedAudioType::F32(self.data),
            sample_rate: self.sample_rate,
            channels,
            frames,
        }
    }
}

/// A resource of raw audio samples stored in deinterleaved format.
///
/// This struct stores samples
/// in their native sample format when possible to save memory.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    resource_type: DecodedAudioType,
    sample_rate: u32,
    channels: usize,
    frames: usize,
}

/// The format of the raw audio samples stored in deinterleaved format.
///
/// Note that there is no option for U32/I32. This is because in processing
/// we ultimately use `f32` for everything anyway. We only store the other
/// types to save memory.
///
/// Also note there is no option for `I24` (signed 24 bit). This is because
/// converting two's compliment numbers from 32 bit to 24 bit and vice-versa
/// is tricky and less performant. Instead, just convert the data into `u24`
/// format.
#[derive(Clone)]
pub enum DecodedAudioType {
    U8(Vec<Vec<u8>>),
    U16(Vec<Vec<u16>>),
    /// The endianness of the samples must be the native endianness of the
    /// target platform.
    U24(Vec<Vec<[u8; 3]>>),
    S8(Vec<Vec<i8>>),
    S16(Vec<Vec<i16>>),
    F32(Vec<Vec<f32>>),
    F64(Vec<Vec<f64>>),
}

impl DecodedAudio {
    pub fn new(resource_type: DecodedAudioType, sample_rate: u32) -> Self {
        let (channels, frames) = match &resource_type {
            DecodedAudioType::U8(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::U16(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::U24(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::S8(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::S16(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::F32(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
            DecodedAudioType::F64(b) => {
                let len = b[0].len();

                for ch in b.iter().skip(1) {
                    assert_eq!(ch.len(), len);
                }

                (b.len(), len)
            }
        };

        Self {
            resource_type,
            sample_rate,
            channels,
            frames,
        }
    }

    /// The number of channels in this resource.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// The length of this resource in frames (length of a single channel in
    /// samples).
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The sample rate of this resource in samples per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn get(&self) -> &DecodedAudioType {
        &self.resource_type
    }

    /// Fill the buffer with samples from the given `channel`, starting from the
    /// given `frame`.
    ///
    /// If the length of the buffer exceeds the length of the PCM resource, then
    /// the remaining samples will be filled with zeros.
    ///
    /// This returns the number of frames that were copied into the buffer. (If
    /// this number is less than the length of `buf`, then it means that the
    /// remaining samples were filled with zeros.)
    ///
    /// The will return an error if the given channel does not exist.
    pub fn fill_channel(&self, channel: usize, frame: usize, buf: &mut [f32]) -> Result<usize, ()> {
        if channel >= self.channels {
            return Err(());
        }

        if frame >= self.frames {
            // Out of range, fill with zeros instead.
            buf.fill(0.0);
            return Ok(0);
        }

        let fill_frames = if frame + buf.len() > self.frames {
            // Fill the out-of-range part with zeros.
            let fill_frames = self.frames - frame;
            buf[fill_frames..].fill(0.0);
            fill_frames
        } else {
            buf.len()
        };

        let buf_part = &mut buf[0..fill_frames];

        match &self.resource_type {
            DecodedAudioType::U8(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_part[i] = FromSample::from_sample(pcm_part[i]);
                }
            }
            DecodedAudioType::U16(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_part[i] = FromSample::from_sample(pcm_part[i]);
                }
            }
            DecodedAudioType::U24(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    let b = &pcm_part[i];

                    let s = if cfg!(target_endian = "little") {
                        // In little-endian the MSB is the last byte.
                        u24(u32::from_le_bytes([b[0], b[1], b[2], 0]))
                    } else {
                        // In big-endian the MSB is the first byte.
                        u24(u32::from_be_bytes([0, b[1], b[2], b[3]]))
                    };

                    buf_part[i] = FromSample::from_sample(s);
                }
            }
            DecodedAudioType::S8(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_part[i] = FromSample::from_sample(pcm_part[i]);
                }
            }
            DecodedAudioType::S16(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_part[i] = FromSample::from_sample(pcm_part[i]);
                }
            }
            DecodedAudioType::F32(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                buf_part.copy_from_slice(pcm_part);
            }
            DecodedAudioType::F64(pcm) => {
                let pcm_part = &pcm[channel][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_part[i] = pcm_part[i] as f32;
                }
            }
        }

        Ok(fill_frames)
    }

    /// Fill the stereo buffer with samples, starting from the given `frame`.
    ///
    /// If this resource has only one channel, then both channels will be
    /// filled with the same data.
    ///
    /// If the length of the buffer exceeds the length of the PCM resource, then
    /// the remaining samples will be filled with zeros.
    ///
    /// This returns the number of frames that were copied into the buffer. (If
    /// this number is less than the length of `buf`, then it means that the
    /// remaining samples were filled with zeros.)
    pub fn fill_stereo(&self, frame: usize, buf_l: &mut [f32], buf_r: &mut [f32]) -> usize {
        let buf_len = buf_l.len().min(buf_r.len());

        if self.channels == 1 {
            let fill_frames = self.fill_channel(0, frame, buf_l).unwrap();
            buf_r.copy_from_slice(buf_l);
            return fill_frames;
        }

        if frame >= self.frames {
            // Out of range, fill with zeros instead.
            buf_l.fill(0.0);
            buf_r.fill(0.0);
            return 0;
        }

        let fill_frames = if frame + buf_len > self.frames {
            // Fill the out-of-range part with zeros.
            let fill_frames = self.frames - frame;
            buf_l[fill_frames..].fill(0.0);
            buf_r[fill_frames..].fill(0.0);
            fill_frames
        } else {
            buf_len
        };

        let buf_l_part = &mut buf_l[0..fill_frames];
        let buf_r_part = &mut buf_r[0..fill_frames];

        match &self.resource_type {
            DecodedAudioType::U8(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_l_part[i] = FromSample::from_sample(pcm_l_part[i]);
                    buf_r_part[i] = FromSample::from_sample(pcm_r_part[i]);
                }
            }
            DecodedAudioType::U16(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_l_part[i] = FromSample::from_sample(pcm_l_part[i]);
                    buf_r_part[i] = FromSample::from_sample(pcm_r_part[i]);
                }
            }
            DecodedAudioType::U24(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    let b_l = &pcm_l_part[i];
                    let b_r = &pcm_r_part[i];

                    let (s_l, s_r) = if cfg!(target_endian = "little") {
                        // In little-endian the MSB is the last byte. Drop it.
                        (
                            u24(u32::from_le_bytes([b_l[0], b_l[1], b_l[2], 0])),
                            u24(u32::from_le_bytes([b_r[0], b_r[1], b_r[2], 0])),
                        )
                    } else {
                        // In big-endian the MSB is the first byte. Drop it.
                        (
                            u24(u32::from_be_bytes([0, b_l[1], b_l[2], b_l[3]])),
                            u24(u32::from_be_bytes([0, b_r[1], b_r[2], b_r[3]])),
                        )
                    };

                    buf_l_part[i] = FromSample::from_sample(s_l);
                    buf_r_part[i] = FromSample::from_sample(s_r);
                }
            }
            DecodedAudioType::S8(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_l_part[i] = FromSample::from_sample(pcm_l_part[i]);
                    buf_r_part[i] = FromSample::from_sample(pcm_r_part[i]);
                }
            }
            DecodedAudioType::S16(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_l_part[i] = FromSample::from_sample(pcm_l_part[i]);
                    buf_r_part[i] = FromSample::from_sample(pcm_r_part[i]);
                }
            }
            DecodedAudioType::F32(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                buf_l_part.copy_from_slice(pcm_l_part);
                buf_r_part.copy_from_slice(pcm_r_part);
            }
            DecodedAudioType::F64(pcm) => {
                let pcm_l_part = &pcm[0][frame..frame + fill_frames];
                let pcm_r_part = &pcm[1][frame..frame + fill_frames];

                for i in 0..fill_frames {
                    buf_l_part[i] = pcm_l_part[i] as f32;
                    buf_r_part[i] = pcm_r_part[i] as f32;
                }
            }
        }

        fill_frames
    }

    /// Consume this resource and return the raw samples.
    pub fn into_raw(self) -> DecodedAudioType {
        self.resource_type
    }
}

impl std::fmt::Debug for DecodedAudioType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::U8(c) => write!(
                f,
                "DecodedAudioType::U8 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::U16(c) => write!(
                f,
                "DecodedAudioType::U16 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::U24(c) => write!(
                f,
                "DecodedAudioType::U24 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::S8(c) => write!(
                f,
                "DecodedAudioType::S8 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::S16(c) => write!(
                f,
                "DecodedAudioType::S16 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::F32(c) => write!(
                f,
                "DecodedAudioType::F32 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
            Self::F64(c) => write!(
                f,
                "DecodedAudioType::F64 {{ channels: {}, frames: {} }}",
                c.len(),
                c[0].len()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_fill_range_test() {
        let test_pcm =
            DecodedAudio::new(DecodedAudioType::F32(vec![vec![1.0, 2.0, 3.0, 4.0]]), 44100);

        let mut out_buf: [f32; 8] = [10.0; 8];

        let fill_frames = test_pcm.fill_channel(0, 0, &mut out_buf[0..4]);
        assert_eq!(fill_frames, Ok(4));
        assert_eq!(&out_buf[0..4], &[1.0, 2.0, 3.0, 4.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 0, &mut out_buf[0..5]);
        assert_eq!(fill_frames, Ok(4));
        assert_eq!(&out_buf[0..5], &[1.0, 2.0, 3.0, 4.0, 0.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 2, &mut out_buf[0..4]);
        assert_eq!(fill_frames, Ok(2));
        assert_eq!(&out_buf[0..4], &[3.0, 4.0, 0.0, 0.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 3, &mut out_buf[0..4]);
        assert_eq!(fill_frames, Ok(1));
        assert_eq!(&out_buf[0..4], &[4.0, 0.0, 0.0, 0.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 4, &mut out_buf[0..4]);
        assert_eq!(fill_frames, Ok(0));
        assert_eq!(&out_buf[0..4], &[0.0, 0.0, 0.0, 0.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 1, &mut out_buf[0..2]);
        assert_eq!(fill_frames, Ok(2));
        assert_eq!(&out_buf[0..2], &[2.0, 3.0]);

        out_buf = [10.0; 8];
        let fill_frames = test_pcm.fill_channel(0, 1, &mut out_buf[0..4]);
        assert_eq!(fill_frames, Ok(3));
        assert_eq!(&out_buf[0..4], &[2.0, 3.0, 4.0, 0.0]);
    }
}
