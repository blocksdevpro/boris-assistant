use std::sync::mpsc;

use boris_audio::AUDIO_TARGET_RATE;
use boris_audio::recorder::{RecordCommand, Recorder};
use boris_core::{AudioBuffer, event::Event, types::ArcAudioBuffer};
use boris_inference::{
    f32_to_pcm16_samples,
    vad::{VadCommand, VadWorker, WebRtcVad},
    wakeword::{LivekitWakeWord, WakeWordCommand, WakeWordWorker},
};

use crate::workers::audio::{AudioDispatcherWorker, AudioPipelineWorker};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

mod workers;

// write a func to save audio in file.wav
fn save_pcm_to_wav(audio: &[i16], filename: &str) {
    use hound::{SampleFormat, WavSpec, WavWriter};

    let spec = WavSpec {
        channels: 1,
        sample_rate: AUDIO_TARGET_RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut writer = WavWriter::create(filename, spec).unwrap();

    for sample in audio {
        writer.write_sample(*sample).unwrap();
    }

    writer.finalize().unwrap();
}

fn main() {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioBuffer>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    let (wakeword_audio_tx, wakeword_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (vad_audio_tx, vad_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (recorder_audio_tx, recorder_audio_rx) = mpsc::channel::<ArcAudioBuffer>();

    let (wakeword_control_tx, wakeword_control_rx) = mpsc::channel::<WakeWordCommand>();
    let (recorder_control_tx, recorder_control_rx) = mpsc::channel::<RecordCommand>();
    let (vad_control_tx, vad_control_rx) = mpsc::channel::<VadCommand>();

    let wakeword = LivekitWakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let vad = WebRtcVad::new();

    let _audio_pipeline_worker = AudioPipelineWorker::spawn(audio_tx);
    let _audio_dispatcer_worker = AudioDispatcherWorker::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );
    let _wakeword_worker = WakeWordWorker::spawn(
        wakeword_audio_rx,
        wakeword_control_rx,
        event_tx.clone(),
        wakeword,
    );
    let _vad_worker = VadWorker::spawn(vad_audio_rx, vad_control_rx, event_tx.clone(), vad);
    let _recorder_worker =
        Recorder::spawn(recorder_audio_rx, recorder_control_rx, event_tx.clone());

    wakeword_control_tx
        .send(WakeWordCommand::StartListening)
        .ok();

    loop {
        if let Ok(event) = event_rx.recv() {
            match event {
                Event::WakeWordDetected => {
                    println!("[BORIS] wakeword detected");
                    wakeword_control_tx
                        .send(WakeWordCommand::StopListening)
                        .ok();
                    vad_control_tx.send(VadCommand::StartListening).ok();
                    recorder_control_tx.send(RecordCommand::StartRecording).ok();
                }
                Event::SpeechEnded => {
                    println!("[BORIS] speech ended");
                    vad_control_tx.send(VadCommand::StopListening).ok();
                    recorder_control_tx.send(RecordCommand::StopRecording).ok();
                }
                Event::RecordingFinished(audio_chunk) => {
                    save_pcm_to_wav(&f32_to_pcm16_samples(&audio_chunk), "output.wav");
                    println!("[BORIS] recording finished");
                    wakeword_control_tx
                        .send(WakeWordCommand::StartListening)
                        .ok();
                }
            }
        }
    }
}
