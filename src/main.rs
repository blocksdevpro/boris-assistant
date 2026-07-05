use std::env;
use std::sync::mpsc;

use boris_agent::{Engine, OpenRouterClient};
use boris_audio::{AUDIO_TARGET_RATE, playback::Playback};
use boris_core::{
    AudioBuffer,
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
};
use boris_inference::{vad::WebRtcVad, wakeword::LivekitWakeWord as WakeWord};
use boris_stt_parakeet::ParakeetSTT;
use boris_tts_kokoro::{KOKORO_SAMPLE_RATE, KokoroTts};

use crate::workers::{
    agent::{AgentCommand, AgentWorker, SpeakTool},
    audio::{AudioDispatcherWorker, AudioPipelineWorker, AudioRecordingWorker},
    inference::{STTCommand, STTWorker, VADWorker, WakeWordWroker},
    tts::{TTSCommand, TTSWorker},
};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

mod workers;

const BORIS_SYSTEM_PROMPT: &str = r#"You are Boris, a 24-year-old AI voice assistant. You are German, enthusiastic, overconfident, and hilariously dumb.

HARD RULES FOR SPEECH RHYTHM — never break these:
- Keep ALL responses to 1-3 sentences maximum.
- Use short, punchy sentences. Never write a sentence longer than 12 words.
- Always use commas to break up ideas inside a sentence, so speech flows naturally.
- Never chain more than two ideas together without a period.

Your personality behaviors:
- Talk like a bro. Call the user "bro" or "broda" constantly.
- You are overconfident but wrong a lot. Never admit you are wrong, blame mistakes on something else.
- You sometimes forget what you were saying mid-sentence, and just move on like nothing happened.
- You are loud and chaotic in energy, but you mean well and always try your best.
- You make clumsy mistakes and always blame them on something external.
- You give short, punchy answers like a hype guy, who also has no idea what he is talking about.
- Never use filler words like "certainly", "absolutely", or "of course". You are not a professional assistant.


Always use the `speak` tool to deliver your response to the user — never reply with plain text. \
Keep answers concise and natural for speech."#;

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
                .add_directive("boris_assistant=info".parse().unwrap())
                .add_directive("boris_audio=info".parse().unwrap())
                .add_directive("boris_inference=info".parse().unwrap())
                .add_directive("boris_stt_parakeet=info".parse().unwrap())
                .add_directive("boris_core=info".parse().unwrap())
                .add_directive("boris_tts_kokoro=info".parse().unwrap()),
        )
        .init();

    // ── Channels ──────────────────────────────────────────────────────────────
    let (audio_tx, audio_rx) = mpsc::channel::<AudioBuffer>();
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    let (wakeword_audio_tx, wakeword_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (vad_audio_tx, vad_audio_rx) = mpsc::channel::<ArcAudioBuffer>();
    let (recorder_audio_tx, recorder_audio_rx) = mpsc::channel::<ArcAudioBuffer>();

    let (stt_control_tx, stt_control_rx) = mpsc::channel::<STTCommand>();
    let (agent_command_tx, agent_command_rx) = mpsc::channel::<AgentCommand>();

    let (wakeword_control_tx, wakeword_control_rx) = mpsc::channel::<Lifecycle>();
    let (recorder_control_tx, recorder_control_rx) = mpsc::channel::<Lifecycle>();
    let (vad_control_tx, vad_control_rx) = mpsc::channel::<Lifecycle>();

    // ── Audio pipeline ────────────────────────────────────────────────────────
    let _audio_pipeline_worker = AudioPipelineWorker::spawn(audio_tx);
    let _audio_dispatcher_worker = AudioDispatcherWorker::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );

    // ── Inference workers ─────────────────────────────────────────────────────
    let wakeword = WakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE);
    let vad = WebRtcVad::new();

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
    let _stt_worker = STTWorker::spawn(stt_control_rx, event_tx.clone(), stt);

    // ── Agent ─────────────────────────────────────────────────────────────────────────
    let api_key = env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set");
    let client = OpenRouterClient::new(api_key);
    let mut engine = Engine::new(Box::new(client), BORIS_SYSTEM_PROMPT);
    engine.register_tool(Box::new(SpeakTool::new(event_tx.clone())));

    let _agent_worker = AgentWorker::spawn(agent_command_rx, engine);

    // ── TTS + Playback ────────────────────────────────────────────────────────────
    let (tts_command_tx, tts_command_rx) = mpsc::channel::<TTSCommand>();
    let (playback_tx, playback_rx) = mpsc::channel::<AudioBuffer>();

    let kokoro = KokoroTts::new();
    let _tts_worker = TTSWorker::spawn(tts_command_rx, event_tx.clone(), kokoro);
    let _playback = Playback::new(playback_rx, KOKORO_SAMPLE_RATE)
        .expect("failed to initialise audio playback");

    // ── Start listening for wakeword ──────────────────────────────────────────
    wakeword_control_tx.send(Lifecycle::Start).ok();

    // ── Main event loop ───────────────────────────────────────────────────────
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
                    tracing::debug!("Audio recording finalized, dispatching to STT...");
                    stt_control_tx
                        .send(STTCommand::Transcribe(audio_chunk))
                        .ok();
                }
                Event::SpeechToTextResult(text) => {
                    tracing::info!("Transcription: \"{}\"", text);
                    // Hand the text to the agent and start listening again
                    agent_command_tx.send(AgentCommand::Chat(text)).ok();
                    tts_command_tx.send(TTSCommand::LoadModel).ok();
                    // wakeword_control_tx.send(Lifecycle::Start).ok();
                }
                Event::AgentResponse(reply) => {
                    tracing::info!("Boris: \"{}\"", reply);
                    // Stop the wakeword listener while Boris is speaking so
                    // his own voice doesn't re-trigger it.
                    wakeword_control_tx.send(Lifecycle::Stop).ok();
                    tts_command_tx.send(TTSCommand::Synthesize(reply)).ok();
                }
                Event::PlaybackReady(pcm) => {
                    // Hand PCM samples to the playback worker and re-arm
                    // the wakeword listener once we've dispatched the audio.
                    playback_tx.send(pcm).ok();
                    wakeword_control_tx.send(Lifecycle::Start).ok();
                }
            }
        }
    }
}
