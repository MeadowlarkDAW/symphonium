// This example loads an audio file and plays it through the system's audio
// output.
//
// This version showcases converting the sample rate of the file during load.

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SupportedBufferSize,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use symphonium::{DecodedAudio, SymphoniumLoader};

pub fn main() {
    simple_log::quick!("info");

    let args: Vec<String> = std::env::args().collect();
    if args.len() == 1 {
        println!("usage: cargo run --example play_file_repitched <ratio> <path-to-audio-file>\ne.g. cargo run --example play_file_repitched 0.75 test_files/synth_keys_44100.ogg");
        return;
    }

    let ratio = args[1]
        .parse::<f64>()
        .expect(&format!("invalid ratio: {}", &args[1]));

    let mut file_path = std::env::current_dir().unwrap();
    file_path.push(&args[2]);

    let host = cpal::default_host();
    let device = host.default_output_device().unwrap();
    let config = device.default_output_config().unwrap();

    let sample_rate = config.sample_rate().0;
    let channels = config.channels() as usize;
    let max_buffer_size = match config.buffer_size() {
        SupportedBufferSize::Range { max, .. } => *max,
        SupportedBufferSize::Unknown => 8192,
    } as usize;
    assert!(channels == 2);

    log::info!("Selected stream sample rate: {}", sample_rate);

    let mut loader = SymphoniumLoader::new();
    let audio_data: DecodedAudio = loader
        .load_f32_stretched(
            file_path,
            ratio,
            Some(sample_rate), // target_sample_rate
            None,              // max_bytes
        )
        .unwrap()
        .into();

    dbg!(&audio_data);

    let mut frames_elapsed = 0;

    let finished_playing = Arc::new(AtomicBool::new(false));
    let finished_playing_1 = Arc::clone(&finished_playing);
    let finished_playing_2 = Arc::clone(&finished_playing);

    let mut temp_buf_l = vec![0.0; max_buffer_size];
    let mut temp_buf_r = vec![0.0; max_buffer_size];

    let stream = device
        .build_output_stream(
            &config.config(),
            move |output: &mut [f32], _: &cpal::OutputCallbackInfo| {
                process(
                    output,
                    &audio_data,
                    &mut frames_elapsed,
                    &mut temp_buf_l,
                    &mut temp_buf_r,
                    &finished_playing_1,
                )
            },
            move |e| {
                log::error!("an error occured on stream: {}", e);
                finished_playing_2.store(true, Ordering::Relaxed);
            },
            None,
        )
        .unwrap();
    stream.play().unwrap();

    while !finished_playing.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn process(
    output: &mut [f32],
    audio_data: &DecodedAudio,
    frames_elapsed: &mut usize,
    temp_buf_l: &mut [f32],
    temp_buf_r: &mut [f32],
    finished_playing: &Arc<AtomicBool>,
) {
    let frames = output.len() / 2;

    audio_data.fill_stereo(
        *frames_elapsed,
        &mut temp_buf_l[..frames],
        &mut temp_buf_r[..frames],
    );

    // Interleave the data into the output.
    for (out, (&in1, &in2)) in output
        .chunks_exact_mut(2)
        .zip(temp_buf_l.iter().zip(temp_buf_r.iter()))
    {
        out[0] = in1;
        out[1] = in2;
    }

    *frames_elapsed += frames;
    if *frames_elapsed >= audio_data.frames() {
        finished_playing.store(true, Ordering::Relaxed);
    }
}
