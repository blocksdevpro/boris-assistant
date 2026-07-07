use dotenvy::dotenv;
use std::env;
use std::sync::mpsc;

use boris_agent::{AgentEngine, OpenRouterClient};
use boris_audio::{playback::Playback, AUDIO_TARGET_RATE};
use boris_core::{
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
    AudioBuffer,
};
use boris_inference::{vad::WebRtcVad, wakeword::LivekitWakeWord as WakeWord};
use boris_stt_parakeet::ParakeetSTT;
use boris_tts_kokoro::{KokoroTts, KOKORO_SAMPLE_RATE};

use crate::workers::{
    agent::{AgentCommand, AgentWorker, SpeakTool},
    audio::{AudioDispatcherWorker, AudioPipelineWorker, AudioRecordingWorker},
    inference::{SttCommand, SttWorker, VadWorker, WakeWordWorker},
    tts::{TtsCommand, TtsWorker},
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
- Talk like a bro. Call the user "bro" constantly.
- You are overconfident but wrong a lot. Never admit you are wrong, blame mistakes on something else.
- You sometimes forget what you were saying mid-sentence, and just move on like nothing happened.
- You are loud and chaotic in energy, but you mean well and always try your best.
- You make clumsy mistakes and always blame them on something external.
- You give short, punchy answers like a hype guy, who also has no idea what he is talking about.
- Never use filler words like "certainly", "absolutely", or "of course". You are not a professional assistant.

Always use the `speak` tool to deliver your response to the user — never reply with plain text. \
Keep answers concise and natural for speech."#;

fn main() {
    // ── Environment & logging ─────────────────────────────────────────────────
    dotenv().ok();
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

    let (stt_cmd_tx, stt_cmd_rx) = mpsc::channel::<SttCommand>();
    let (agent_cmd_tx, agent_cmd_rx) = mpsc::channel::<AgentCommand>();
    let (tts_cmd_tx, tts_cmd_rx) = mpsc::channel::<TtsCommand>();
    let (playback_tx, playback_rx) = mpsc::channel::<AudioBuffer>();

    let (wakeword_ctl_tx, wakeword_ctl_rx) = mpsc::channel::<Lifecycle>();
    let (recorder_ctl_tx, recorder_ctl_rx) = mpsc::channel::<Lifecycle>();
    let (vad_ctl_tx, vad_ctl_rx) = mpsc::channel::<Lifecycle>();

    // ── Audio pipeline ────────────────────────────────────────────────────────
    let _audio_pipeline =
        AudioPipelineWorker::spawn(audio_tx).expect("failed to initialise audio capture");

    let _audio_dispatcher = AudioDispatcherWorker::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );

    // ── Inference workers ─────────────────────────────────────────────────────
    let _wakeword_worker = WakeWordWorker::spawn(
        wakeword_audio_rx,
        wakeword_ctl_rx,
        event_tx.clone(),
        WakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE),
    );

    let _vad_worker =
        VadWorker::spawn(vad_audio_rx, vad_ctl_rx, event_tx.clone(), WebRtcVad::new());

    let _recording_worker =
        AudioRecordingWorker::spawn(recorder_audio_rx, recorder_ctl_rx, event_tx.clone());

    let _stt_worker = SttWorker::spawn(stt_cmd_rx, event_tx.clone(), ParakeetSTT::new());

    // ── Agent ─────────────────────────────────────────────────────────────────
    let api_key = env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set");
    let client = OpenRouterClient::new(api_key);
    let mut engine = AgentEngine::new(Box::new(client), BORIS_SYSTEM_PROMPT);
    engine.register_tool(Box::new(SpeakTool::new(event_tx.clone())));

    let _agent_worker = AgentWorker::spawn(agent_cmd_rx, engine);

    // ── TTS + Playback ────────────────────────────────────────────────────────
    let _tts_worker = TtsWorker::spawn(tts_cmd_rx, event_tx.clone(), KokoroTts::new());
    let _playback = Playback::new(playback_rx, KOKORO_SAMPLE_RATE)
        .expect("failed to initialise audio playback");

    // ── Start listening for the wake word ─────────────────────────────────────
    wakeword_ctl_tx.send(Lifecycle::Start).ok();

    // ── Main event loop ───────────────────────────────────────────────────────
    tracing::info!("Boris is ready. Say the wake word to begin.");

    while let Ok(event) = event_rx.recv() {
        match event {
            Event::WakeWordDetected => {
                tracing::info!("Wake word detected — listening…");
                wakeword_ctl_tx.send(Lifecycle::Stop).ok();
                vad_ctl_tx.send(Lifecycle::Start).ok();
                recorder_ctl_tx.send(Lifecycle::Start).ok();
                stt_cmd_tx.send(SttCommand::LoadModel).ok();
            }

            Event::SpeechEnded => {
                tracing::info!("Speech ended — transcribing…");
                vad_ctl_tx.send(Lifecycle::Stop).ok();
                recorder_ctl_tx.send(Lifecycle::Stop).ok();
            }

            Event::RecordingResult(audio) => {
                tracing::debug!("Recording finalised — dispatching to STT");
                stt_cmd_tx.send(SttCommand::Transcribe(audio)).ok();
            }

            Event::SpeechToTextResult(text) => {
                tracing::info!(text, "Transcription complete");
                agent_cmd_tx.send(AgentCommand::Chat(text)).ok();
                tts_cmd_tx.send(TtsCommand::LoadModel).ok();
            }

            Event::AgentResponse(reply) => {
                tracing::info!(reply, "Boris speaking");
                // Stop the wakeword listener while Boris is speaking so his
                // own voice doesn't re-trigger detection.
                wakeword_ctl_tx.send(Lifecycle::Stop).ok();
                tts_cmd_tx.send(TtsCommand::Synthesize(reply)).ok();
            }

            Event::PlaybackReady(pcm) => {
                playback_tx.send(pcm).ok();
                // Re-arm wakeword detection once audio is dispatched.
                wakeword_ctl_tx.send(Lifecycle::Start).ok();
            }

            Event::WorkerError { worker, message } => {
                tracing::error!(worker, message, "worker error");
                // Re-arm the wakeword so the assistant doesn't get stuck.
                wakeword_ctl_tx.send(Lifecycle::Start).ok();
            }
        }
    }
}
