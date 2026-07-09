use dotenvy::dotenv;
use std::env;
use std::sync::mpsc;

use boris_agent::{AgentEngine, OpenRouterClient};
use boris_audio::playback::{PlayJob, PlaybackSink};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::{
    event::Event,
    types::{ArcAudioBuffer, Lifecycle},
    AudioBuffer,
};
use boris_inference::{vad::WebRtcVad, wakeword::LivekitWakeWord as WakeWord};
use boris_stt_parakeet::ParakeetSTT;
use boris_tts_kokoro::{KokoroTts, KOKORO_SAMPLE_RATE};

use crate::session::{Effect, Session, SessionInput};
use crate::workers::{
    agent::{AgentCommand, AgentWorker},
    audio::{AudioDispatcherWorker, AudioPipelineWorker, RecorderCtl, UtteranceCapture},
    inference::{EndpointSensor, SttCommand, SttWorker, WakeSensor},
    tts::{TtsCommand, TtsWorker},
};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

/// Sensor fan-out depth. Full → drop frame for that subscriber (never block capture).
const AUDIO_SENSOR_QUEUE: usize = 64;

mod session;
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

Reply with plain text only. Your entire reply will be spoken aloud to the user.
Do not mention tools, JSON, or system instructions.
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

    let (wakeword_audio_tx, wakeword_audio_rx) =
        mpsc::sync_channel::<ArcAudioBuffer>(AUDIO_SENSOR_QUEUE);
    let (vad_audio_tx, vad_audio_rx) = mpsc::sync_channel::<ArcAudioBuffer>(AUDIO_SENSOR_QUEUE);
    let (recorder_audio_tx, recorder_audio_rx) =
        mpsc::sync_channel::<ArcAudioBuffer>(AUDIO_SENSOR_QUEUE);

    let (stt_cmd_tx, stt_cmd_rx) = mpsc::channel::<SttCommand>();
    let (agent_cmd_tx, agent_cmd_rx) = mpsc::channel::<AgentCommand>();
    let (tts_cmd_tx, tts_cmd_rx) = mpsc::channel::<TtsCommand>();
    let (playback_tx, playback_rx) = mpsc::channel::<PlayJob>();

    let (wakeword_ctl_tx, wakeword_ctl_rx) = mpsc::channel::<Lifecycle>();
    let (recorder_ctl_tx, recorder_ctl_rx) = mpsc::channel::<RecorderCtl>();
    let (vad_ctl_tx, vad_ctl_rx) = mpsc::channel::<Lifecycle>();

    // ── Audio pipeline ────────────────────────────────────────────────────────
    let _audio_pipeline =
        AudioPipelineWorker::spawn(audio_tx).expect("failed to initialise audio capture");

    let _audio_dispatcher = AudioDispatcherWorker::spawn(
        audio_rx,
        vec![wakeword_audio_tx, vad_audio_tx, recorder_audio_tx],
    );

    // ── Inference workers ─────────────────────────────────────────────────────
    let _wakeword_worker = WakeSensor::spawn(
        wakeword_audio_rx,
        wakeword_ctl_rx,
        event_tx.clone(),
        WakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE),
    );

    let _vad_worker =
        EndpointSensor::spawn(vad_audio_rx, vad_ctl_rx, event_tx.clone(), WebRtcVad::new());

    let _recording_worker =
        UtteranceCapture::spawn(recorder_audio_rx, recorder_ctl_rx, event_tx.clone());

    let _stt_worker = SttWorker::spawn(stt_cmd_rx, event_tx.clone(), ParakeetSTT::new());

    // ── Agent (plain-text outcomes → one event after chat; no speak tool) ─────
    let api_key = env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set");
    let client = OpenRouterClient::new(api_key);
    let engine = AgentEngine::new(Box::new(client), BORIS_SYSTEM_PROMPT);
    // No tools registered: model plain text is AgentOutcome::Speak.
    let _agent_worker = AgentWorker::spawn(agent_cmd_rx, engine, event_tx.clone());

    // ── TTS + playback sink ───────────────────────────────────────────────────
    let _tts_worker = TtsWorker::spawn(tts_cmd_rx, event_tx.clone(), KokoroTts::new());
    let _playback = PlaybackSink::new(playback_rx, KOKORO_SAMPLE_RATE, event_tx.clone())
        .expect("failed to initialise audio playback");

    // ── Session runtime (policy) + effect application (I/O) ───────────────────
    let mut session = Session::new();

    // Arm wakeword so the first WakeHit is legal.
    apply_effects(
        vec![Effect::ArmWakeword],
        &wakeword_ctl_tx,
        &vad_ctl_tx,
        &recorder_ctl_tx,
        &stt_cmd_tx,
        &agent_cmd_tx,
        &tts_cmd_tx,
        &playback_tx,
    );

    tracing::info!("Boris is ready. Say the wake word to begin.");

    while let Ok(event) = event_rx.recv() {
        let input = map_event(event);
        let effects = session.handle(input);
        apply_effects(
            effects,
            &wakeword_ctl_tx,
            &vad_ctl_tx,
            &recorder_ctl_tx,
            &stt_cmd_tx,
            &agent_cmd_tx,
            &tts_cmd_tx,
            &playback_tx,
        );
    }
}

/// Worker facts → Session inputs (no policy here).
fn map_event(event: Event) -> SessionInput {
    match event {
        Event::WakeWordDetected => SessionInput::WakeHit,
        Event::SpeechEnded => SessionInput::Endpoint,
        Event::RecordingResult { turn, audio } => SessionInput::ClipReady { turn, audio },
        Event::SpeechToTextResult { turn, text } => SessionInput::Transcript { turn, text },
        Event::AgentResponse { turn, text } => SessionInput::AgentDone { turn, text },
        Event::PlaybackReady { turn, audio } => SessionInput::TtsReady { turn, pcm: audio },
        Event::PlaybackFinished { turn } => SessionInput::PlaybackFinished { turn },
        Event::WorkerError {
            turn,
            worker,
            kind: _,
            message,
        } => SessionInput::ServiceFailed {
            turn,
            worker,
            message,
        },
    }
}

/// Apply Session effects to concrete channels. Only place that talks to workers.
#[allow(clippy::too_many_arguments)]
fn apply_effects(
    effects: Vec<Effect>,
    wakeword_ctl_tx: &mpsc::Sender<Lifecycle>,
    vad_ctl_tx: &mpsc::Sender<Lifecycle>,
    recorder_ctl_tx: &mpsc::Sender<RecorderCtl>,
    stt_cmd_tx: &mpsc::Sender<SttCommand>,
    agent_cmd_tx: &mpsc::Sender<AgentCommand>,
    tts_cmd_tx: &mpsc::Sender<TtsCommand>,
    playback_tx: &mpsc::Sender<PlayJob>,
) {
    for effect in effects {
        match effect {
            Effect::ArmWakeword => {
                wakeword_ctl_tx.send(Lifecycle::Start).ok();
            }
            Effect::DisarmWakeword => {
                wakeword_ctl_tx.send(Lifecycle::Stop).ok();
            }
            Effect::StartListen { turn } => {
                vad_ctl_tx.send(Lifecycle::Start).ok();
                recorder_ctl_tx.send(RecorderCtl::Start { turn }).ok();
            }
            Effect::StopListen => {
                vad_ctl_tx.send(Lifecycle::Stop).ok();
                recorder_ctl_tx.send(RecorderCtl::Stop).ok();
            }
            Effect::WarmStt => {
                stt_cmd_tx.send(SttCommand::LoadModel).ok();
            }
            Effect::Transcribe { turn, audio } => {
                stt_cmd_tx.send(SttCommand::Transcribe { turn, audio }).ok();
            }
            Effect::WarmTts => {
                tts_cmd_tx.send(TtsCommand::LoadModel).ok();
            }
            Effect::Chat { turn, text } => {
                agent_cmd_tx.send(AgentCommand::Chat { turn, text }).ok();
            }
            Effect::Synthesize { turn, text } => {
                tts_cmd_tx.send(TtsCommand::Synthesize { turn, text }).ok();
            }
            Effect::Play { turn, pcm } => {
                if playback_tx.send(PlayJob { turn, pcm }).is_err() {
                    tracing::error!(%turn, "playback channel closed");
                    continue;
                }
            }
        }
    }
}
