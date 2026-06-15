use std::sync::mpsc;

use boris_audio::{
    AUDIO_TARGET_RATE,
    pipeline::AudioPipeline,
    processor::AudioProcessor,
    recorder::{AudioRecorder, RecordCommand},
};
use boris_core::{AudioSampleBuffer, event::BorisEvent};
use boris_inference::{
    f32_to_pcm16_samples,
    vad::{BorisVad, BorisVadProcessor, VadCommand},
    wakeword::{BorisWakeWord, BorisWakeWordProcessor, WakeWordCommand},
};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

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
        writer.write_sample(*sample as i16).unwrap();
    }

    writer.finalize().unwrap();
}

fn main() {
    let (audio_tx, audio_rx) = mpsc::channel::<AudioSampleBuffer>();
    let (event_tx, event_rx) = mpsc::channel::<BorisEvent>();

    let (wakeword_audio_tx, wakeword_audio_rx) = mpsc::channel::<AudioSampleBuffer>();
    let (vad_audio_tx, vad_audio_rx) = mpsc::channel::<AudioSampleBuffer>();
    let (recorder_audio_tx, recorder_audio_rx) = mpsc::channel::<AudioSampleBuffer>();

    let (wakeword_control_tx, wakeword_control_rx) = mpsc::channel::<WakeWordCommand>();
    let (recorder_control_tx, recorder_control_rx) = mpsc::channel::<RecordCommand>();
    let (vad_control_tx, vad_control_rx) = mpsc::channel::<VadCommand>();

    let wakeword = BorisWakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let vad = BorisVad::new();

    let _audio_pipeline = AudioPipeline::spawn(audio_tx);
    let _audio_processor = AudioProcessor::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );
    let _wakeword_processor = BorisWakeWordProcessor::spawn(
        wakeword_audio_rx,
        wakeword_control_rx,
        event_tx.clone(),
        wakeword,
    );
    let _vad_processor =
        BorisVadProcessor::spawn(vad_audio_rx, vad_control_rx, event_tx.clone(), vad);
    let _recorder_processor =
        AudioRecorder::spawn(recorder_audio_rx, recorder_control_rx, event_tx.clone());

    wakeword_control_tx
        .send(WakeWordCommand::StartListening)
        .ok();

    loop {
        if let Ok(event) = event_rx.recv() {
            match event {
                BorisEvent::WakeWordDetected => {
                    println!("[BORIS] wakeword detected");
                    wakeword_control_tx
                        .send(WakeWordCommand::StopListening)
                        .ok();
                    vad_control_tx.send(VadCommand::StartListening).ok();
                    recorder_control_tx.send(RecordCommand::StartRecording).ok();
                }
                BorisEvent::SpeechEnded => {
                    println!("[BORIS] speech ended");
                    vad_control_tx.send(VadCommand::StopListening).ok();
                    recorder_control_tx.send(RecordCommand::StopRecording).ok();
                }
                BorisEvent::RecordingFinished(audio_chunk) => {
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
