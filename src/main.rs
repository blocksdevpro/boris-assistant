use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::TurnId;
use boris_tts_supertone::{SupertoneTts, SUPERTONE_SAMPLE_RATE};
use dotenvy::dotenv;
use std::sync::{mpsc, Arc, Mutex};
use std::{env, thread};

use boris_agent::{AgentEngine, OpenRouterClient};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::{event::Event, types::Lifecycle};
use boris_inference::{init_onnx_runtime, vad::WebRtcVad, wakeword::LivekitWakeWord as WakeWord};
use boris_stt_parakeet::ParakeetSTT;

use crate::session::{Effect, Session, SessionInput};
use crate::workers::{
    agent::{AgentCommand, AgentWorker},
    audio::{RecorderCtl, UtteranceCapture},
    inference::{EndpointSensor, SttCommand, SttWorker, WakeSensor},
    tts::{TtsCommand, TtsWorker},
};

static WAKEWORD_MODEL_BYTES: &[u8] = include_bytes!("../assets/models/livekit/boris-large.onnx");

/// Sensor fan-out depth. Full → drop frame for that subscriber (never block capture).
const AUDIO_SENSOR_QUEUE: usize = 64;

mod prompt;
mod session;
mod workers;

use prompt::BORIS_SYSTEM_PROMPT;

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
                .add_directive("boris_tts_supertone=info".parse().unwrap()),
        )
        .init();

    // Cap ORT thread pools *before* any ONNX sessions (wakeword mel/emb/classifier).
    init_onnx_runtime();

    // Play path resamples from TTS rate → device rate. Must match Supertone (44.1 kHz).
    let mut audio_service = AudioService::with_source_rate(SUPERTONE_SAMPLE_RATE);

    // ── Channels ──────────────────────────────────────────────────────────────
    let (event_tx, event_rx) = mpsc::channel::<Event>();

    let (stt_cmd_tx, stt_cmd_rx) = mpsc::channel::<SttCommand>();
    let (agent_cmd_tx, agent_cmd_rx) = mpsc::channel::<AgentCommand>();
    let (tts_cmd_tx, tts_cmd_rx) = mpsc::channel::<TtsCommand>();

    let (wakeword_ctl_tx, wakeword_ctl_rx) = mpsc::channel::<Lifecycle>();
    let (recorder_ctl_tx, recorder_ctl_rx) = mpsc::channel::<RecorderCtl>();
    let (vad_ctl_tx, vad_ctl_rx) = mpsc::channel::<Lifecycle>();

    // ── Inference workers ─────────────────────────────────────────────────────
    let _wakeword_worker = WakeSensor::spawn(
        audio_service.subscribe_input(Some(AUDIO_SENSOR_QUEUE)),
        wakeword_ctl_rx,
        event_tx.clone(),
        WakeWord::new("boris", WAKEWORD_MODEL_BYTES, AUDIO_TARGET_RATE),
    );

    let _vad_worker = EndpointSensor::spawn(
        audio_service.subscribe_input(Some(AUDIO_SENSOR_QUEUE)),
        vad_ctl_rx,
        event_tx.clone(),
        WebRtcVad::new(),
    );

    let _recording_worker = UtteranceCapture::spawn(
        audio_service.subscribe_input(Some(AUDIO_SENSOR_QUEUE)),
        recorder_ctl_rx,
        event_tx.clone(),
    );

    let _stt_worker = SttWorker::spawn(stt_cmd_rx, event_tx.clone(), ParakeetSTT::new());

    // ── Agent (plain-text outcomes → one event after chat; no speak tool) ─────
    let api_key = env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set");
    let model = env::var("OPENROUTER_MODEL").ok();

    let client = OpenRouterClient::new(api_key, model);
    let engine = AgentEngine::new(Box::new(client), BORIS_SYSTEM_PROMPT);
    // No tools registered: model plain text is AgentOutcome::Speak.
    let _agent_worker = AgentWorker::spawn(agent_cmd_rx, engine, event_tx.clone());

    // ── TTS + playback sink (Supertone) ───────────────────────────────────────
    let _tts_worker = TtsWorker::spawn(tts_cmd_rx, event_tx.clone(), SupertoneTts::new());

    // ── Session runtime (policy) + effect application (I/O) ───────────────────
    let mut session = Session::new();

    // AudioService ouput work-around
    let active_play_turn = Arc::new(Mutex::new(None::<TurnId>));

    let output_event_rx = audio_service.subscribe_output();
    let event_tx_clone = event_tx.clone();
    let active_play_turn_clone = active_play_turn.clone();

    thread::spawn(move || {
        while let Ok(event) = output_event_rx.recv() {
            match event {
                OutputEvent::Drained => {
                    if let Some(turn) = active_play_turn_clone.lock().unwrap().take() {
                        event_tx_clone.send(Event::PlaybackFinished { turn }).ok();
                    }
                }
                OutputEvent::Cleared => {
                    active_play_turn_clone.lock().unwrap().take();
                }
            }
        }
    });

    // Arm wakeword so the first WakeHit is legal.
    apply_effects(
        vec![Effect::ArmWakeword],
        &wakeword_ctl_tx,
        &vad_ctl_tx,
        &recorder_ctl_tx,
        &stt_cmd_tx,
        &agent_cmd_tx,
        &tts_cmd_tx,
        &audio_service,
        &active_play_turn,
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
            &audio_service,
            &active_play_turn,
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
    audio_service: &AudioService,
    active_play_turn: &Arc<Mutex<Option<TurnId>>>,
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
                {
                    let mut active_play_turn = active_play_turn.lock().unwrap();
                    *active_play_turn = Some(turn);
                }
                audio_service.play(pcm);
            }
            Effect::StopPlayback => {
                audio_service.stop();
            }
        }
    }
}
