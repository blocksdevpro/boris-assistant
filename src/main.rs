use std::sync::mpsc;

use boris_audio::AUDIO_TARGET_RATE;
use boris_core::{
    AudioBuffer,
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
};
use boris_inference::{vad::WebRtcVad, wakeword::LivekitWakeWord as WakeWord};
use boris_stt_parakeet::ParakeetSTT;

use crate::workers::{
    audio::{AudioDispatcherWorker, AudioPipelineWorker, AudioRecordingWorker},
    inference::{STTCommand, STTWorker, VADWorker, WakeWordWroker},
};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

mod workers;

// write a func to save audio in file.wav
#[allow(dead_code)]
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into())
                .add_directive("boris_assistant=debug".parse().unwrap())
                .add_directive("boris_audio=info".parse().unwrap())
                .add_directive("boris_inference=debug".parse().unwrap())
                .add_directive("boris_stt_parakeet=info".parse().unwrap())
                .add_directive("boris_core=info".parse().unwrap()),
        )
        .init();

    let (audio_tx, audio_rx) = mpsc::channel::<AudioBuffer>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    let (wakeword_audio_tx, wakeword_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (vad_audio_tx, vad_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (recorder_audio_tx, recorder_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (stt_control_tx, stt_control_rx) = mpsc::channel::<STTCommand>();

    let (wakeword_control_tx, wakeword_control_rx) = mpsc::channel::<Lifecycle>();
    let (recorder_control_tx, recorder_control_rx) = mpsc::channel::<Lifecycle>();
    let (vad_control_tx, vad_control_rx) = mpsc::channel::<Lifecycle>();

    let wakeword = WakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let vad = WebRtcVad::new();

    let _audio_pipeline_worker = AudioPipelineWorker::spawn(audio_tx);
    let _audio_dispatcer_worker = AudioDispatcherWorker::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );
    let _wakeword_worker = WakeWordWroker::spawn(
        wakeword_audio_rx,
        wakeword_control_rx,
        event_tx.clone(),
        wakeword,
    );
    let _vad_worker = VADWorker::spawn(vad_audio_rx, vad_control_rx, event_tx.clone(), vad);
    let _recording_worker =
        AudioRecordingWorker::spawn(recorder_audio_rx, recorder_control_rx, event_tx.clone());

    let stt = ParakeetSTT::new();
    let _stt_worker = STTWorker::spawn(stt_control_rx, event_tx, stt);

    wakeword_control_tx.send(Lifecycle::Start).ok();

    loop {
        if let Ok(event) = event_rx.recv() {
            match event {
                Event::WakeWordDetected => {
                    tracing::info!("Wakeword detected, listening...");
                    wakeword_control_tx.send(Lifecycle::Stop).ok();
                    vad_control_tx.send(Lifecycle::Start).ok();
                    recorder_control_tx.send(Lifecycle::Start).ok();
                    stt_control_tx.send(STTCommand::LoadModel).ok();
                }
                Event::SpeechEnded => {
                    tracing::info!("Speech ended, processing audio...");
                    vad_control_tx.send(Lifecycle::Stop).ok();
                    recorder_control_tx.send(Lifecycle::Stop).ok();
                }
                Event::RecordingResult(audio_chunk) => {
                    tracing::debug!("Audio recording finalized and dispatched to STT");
                    stt_control_tx
                        .send(STTCommand::Transcribe(audio_chunk))
                        .ok();
                }

                Event::SpeechToTextResult(text) => {
                    tracing::info!("Transcription: \"{}\"", text);
                    wakeword_control_tx.send(Lifecycle::Start).ok();
                }
            }
        }
    }
}
