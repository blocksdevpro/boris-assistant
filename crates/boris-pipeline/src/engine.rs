//! Single-threaded sequential voice engine for desktop.
//!
//! One background thread owns audio + models and walks a turn top-to-bottom.
//! Phase updates are for the UI only — control flow is ordinary `?` / `match`.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::context::Context;
use boris_agent::session::store::SessionStore;
use boris_agent::session::types::SessionId;
use boris_agent::{AgentEngine, AgentOutcome, OpenRouterClient};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::TurnId;
use boris_inference::{SpeechToText, TextToSpeech};
use boris_sense::{init_onnx_runtime, LivekitWakeWord, WebRtcVad};

use crate::config::PipelineConfig;
use crate::devices::{find_input, find_output};
use crate::hear::{self, CaptureKind, HearBreak};
use crate::paths;
use crate::status::{DeviceHealth, EngineState, Phase, StatusPicture};

const MIC_QUEUE: usize = 64;
/// Max freeform follow-ups without re-wake (name, choice, full sentence, yes/no, …).
const MAX_FOLLOW_UPS: u32 = 3;

#[derive(Debug)]
pub enum EngineCommand {
    Start,
    Stop,
    Shutdown,
    SwitchInput { device_id: String },
    SwitchOutput { device_id: String },
}

#[derive(Clone)]
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
}

impl EngineHandle {
    pub fn send(&self, cmd: EngineCommand) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.cmd_tx.send(cmd)
    }

    pub fn start(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Start)
    }

    pub fn stop(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Stop)
    }

    pub fn shutdown(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Shutdown)
    }
}

/// Join handle for the engine thread (drop does not join).
pub struct Engine {
    _join: JoinHandle<()>,
}

impl Engine {
    /// Spawn the engine thread. Status snapshots are sent on the returned receiver.
    pub fn spawn(config: PipelineConfig) -> (Self, EngineHandle, Receiver<StatusPicture>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let join = thread::Builder::new()
            .name("boris-engine".into())
            .spawn(move || {
                if let Err(e) = run(config, cmd_rx, status_tx) {
                    tracing::error!(error = %e, "engine thread exited with error");
                }
            })
            .expect("spawn boris-engine");

        (Self { _join: join }, EngineHandle { cmd_tx }, status_rx)
    }
}

struct Picture {
    engine: EngineState,
    phase: Phase,
    detail: Option<String>,
    heard: Option<String>,
    said: Option<String>,
    mic: DeviceHealth,
    speaker: DeviceHealth,
    turn: Option<TurnId>,
    status_tx: Sender<StatusPicture>,
}

impl Picture {
    fn publish(&self) {
        let _ = self.status_tx.send(StatusPicture {
            engine: self.engine,
            phase: self.phase,
            detail: self.detail.clone(),
            heard: self.heard.clone(),
            said: self.said.clone(),
            mic: self.mic.clone(),
            speaker: self.speaker.clone(),
            turn: self.turn.map(|t| t.to_string()),
        });
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.publish();
    }
}

fn run(
    config: PipelineConfig,
    cmd_rx: Receiver<EngineCommand>,
    status_tx: Sender<StatusPicture>,
) -> Result<(), String> {
    init_onnx_runtime();

    let mut audio = match AudioService::with_source_rate(config.play_source_rate) {
        Ok(audio) => audio,
        Err(e) => {
            let _ = status_tx.send(StatusPicture {
                engine: EngineState::Fault,
                phase: Phase::Off,
                detail: Some(e.clone()),
                heard: None,
                said: None,
                mic: DeviceHealth {
                    label: config.mic_label.clone(),
                    ok: false,
                },
                speaker: DeviceHealth {
                    label: config.speaker_label.clone(),
                    ok: false,
                },
                turn: None,
            });
            return Err(format!("audio init failed: {e}"));
        }
    };
    let mic = audio.subscribe_input(Some(MIC_QUEUE));
    let mut output_events = audio.subscribe_output();

    let mut wake = LivekitWakeWord::new(
        "boris",
        &config.wakeword_model,
        boris_audio::AUDIO_TARGET_RATE,
    );
    let mut vad = WebRtcVad::new();

    tracing::info!(
        stt = %config.stt_model_dir.display(),
        tts = %config.tts_model_dir.display(),
        voices = %config.tts_voice_dir.display(),
        "model paths"
    );

    #[cfg(feature = "stt-parakeet")]
    let mut stt: Box<dyn SpeechToText> = Box::new(boris_stt_parakeet::ParakeetStt::with_model_dir(
        config.stt_model_dir.clone(),
    ));
    #[cfg(not(feature = "stt-parakeet"))]
    let mut stt: Box<dyn SpeechToText> = Box::new(NullStt);

    #[cfg(feature = "tts-supertone")]
    let mut tts: Box<dyn TextToSpeech> = Box::new(boris_tts_supertone::SupertoneTts::with_paths(
        config.tts_model_dir.clone(),
        config.tts_voice_dir.clone(),
        &config.tts_voice_id,
    ));
    #[cfg(not(feature = "tts-supertone"))]
    let mut tts: Box<dyn TextToSpeech> = Box::new(NullTts);

    let client = OpenRouterClient::new(config.openrouter_api_key, config.openrouter_model);
    let mut agent = AgentEngine::new(Box::new(client), &config.system_prompt);
    // Time/date, notes, and active personal context (profile tools + extract).
    boris_agent::tools::register_builtin_tools(
        &mut agent,
        boris_agent::tools::BuiltinToolPaths {
            notes_path: paths::notes_path(),
            profile_path: paths::profile_path(),
        },
    );

    // Session persistence under ~/.boris/sessions (soft-fail on I/O).
    if let Err(e) = paths::ensure_sessions_dir() {
        tracing::warn!(error = %e, "ensure sessions dir failed");
    }
    let store = SessionStore::new(paths::sessions_dir());
    let mut active_session: Option<SessionId> = None;

    let mut picture = Picture {
        engine: EngineState::Off,
        phase: Phase::Off,
        detail: None,
        heard: None,
        said: None,
        mic: DeviceHealth {
            label: config.mic_label,
            ok: true,
        },
        speaker: DeviceHealth {
            label: config.speaker_label,
            ok: true,
        },
        turn: None,
        status_tx,
    };
    picture.publish();

    let mut running = false;
    let mut next_turn: u64 = 1;
    /// When true, next iteration skips wake and freeform-listens for a reply.
    let mut await_reply = false;
    let mut follow_up_depth: u32 = 0;

    loop {
        // ── Off: wait for Start ─────────────────────────────────────────────
        if !running {
            await_reply = false;
            follow_up_depth = 0;
            match cmd_rx.recv() {
                Ok(EngineCommand::Start) => {
                    running = true;
                    picture.engine = EngineState::On;
                    picture.detail = None;
                    picture.heard = None;
                    picture.said = None;
                    picture.turn = None;
                    begin_session(
                        &store,
                        &mut active_session,
                        &mut agent,
                        &config.system_prompt,
                    );
                    picture.set_phase(Phase::Armed);
                    tracing::info!("engine started");
                }
                Ok(EngineCommand::Stop) => continue,
                Ok(EngineCommand::Shutdown) | Err(_) => {
                    end_session(&store, &mut active_session);
                    picture.engine = EngineState::Off;
                    picture.set_phase(Phase::Off);
                    return Ok(());
                }
                Ok(EngineCommand::SwitchInput { device_id }) => {
                    apply_input_switch(&mut audio, &mut picture, &device_id);
                }
                Ok(EngineCommand::SwitchOutput { device_id }) => {
                    apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                }
            }
            continue;
        }

        // ── Entry: wake OR freeform follow-up (no second wake) ─────────────
        let capture_kind = if await_reply {
            picture.heard = None;
            // Keep last `said` so UI still shows what Boris asked.
            picture.detail = None;
            picture.turn = None;
            picture.set_phase(Phase::AwaitingReply);
            tracing::info!(
                depth = follow_up_depth,
                "awaiting freeform user reply (no wake)"
            );
            if let Err(e) = hear::settle_after_playback(&mic, &cmd_rx, &mut running) {
                match e {
                    HearBreak::SwitchInput { device_id } => {
                        apply_input_switch(&mut audio, &mut picture, &device_id);
                        continue;
                    }
                    HearBreak::SwitchOutput { device_id } => {
                        apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                        continue;
                    }
                    HearBreak::Stopped if !running => {
                        go_off(&mut picture, &audio, &store, &mut active_session);
                        continue;
                    }
                    HearBreak::Stopped => {
                        await_reply = false;
                        follow_up_depth = 0;
                        continue;
                    }
                    HearBreak::Disconnected => {
                        end_session(&store, &mut active_session);
                        picture.set_phase(Phase::Off);
                        return Ok(());
                    }
                }
            }
            if !running {
                go_off(&mut picture, &audio, &store, &mut active_session);
                continue;
            }
            // Consumed for this entry; may re-arm after this turn if Boris asks again.
            await_reply = false;
            CaptureKind::AwaitReply
        } else {
            follow_up_depth = 0;
            picture.heard = None;
            picture.said = None;
            picture.detail = None;
            picture.turn = None;
            picture.set_phase(Phase::Armed);

            match hear::wait_for_wake(&mic, &mut wake, &cmd_rx, &mut running) {
                Ok(()) => {}
                Err(HearBreak::SwitchInput { device_id }) => {
                    apply_input_switch(&mut audio, &mut picture, &device_id);
                    continue;
                }
                Err(HearBreak::SwitchOutput { device_id }) => {
                    apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                    continue;
                }
                Err(HearBreak::Stopped) if !running => {
                    go_off(&mut picture, &audio, &store, &mut active_session);
                    continue;
                }
                Err(HearBreak::Stopped) => continue,
                Err(HearBreak::Disconnected) => {
                    end_session(&store, &mut active_session);
                    picture.set_phase(Phase::Off);
                    return Ok(());
                }
            }

            if !running {
                go_off(&mut picture, &audio, &store, &mut active_session);
                continue;
            }
            CaptureKind::AfterWake
        };

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId(next_turn);
        next_turn = next_turn.saturating_add(1);
        picture.turn = Some(turn);
        picture.set_phase(Phase::Hearing);
        tracing::info!(%turn, ?capture_kind, "turn begin — hearing");

        let clip = match hear::capture_utterance(&mic, &mut vad, &cmd_rx, &mut running, capture_kind)
        {
            Ok(c) => c,
            Err(HearBreak::SwitchInput { device_id }) => {
                apply_input_switch(&mut audio, &mut picture, &device_id);
                continue;
            }
            Err(HearBreak::SwitchOutput { device_id }) => {
                apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                continue;
            }
            Err(HearBreak::Stopped) if !running => {
                go_off(&mut picture, &audio, &store, &mut active_session);
                continue;
            }
            Err(HearBreak::Stopped) => {
                follow_up_depth = 0;
                continue;
            }
            Err(HearBreak::Disconnected) => {
                end_session(&store, &mut active_session);
                return Ok(());
            }
        };

        if !running {
            go_off(&mut picture, &audio, &store, &mut active_session);
            continue;
        }

        // Read — load STT once and keep warm across turns (no unload on success).
        picture.set_phase(Phase::Reading);
        if let Err(e) = stt.load() {
            tracing::error!(error = %e, %turn, "stt load failed");
            picture.detail = Some(format!("stt load: {e}"));
            follow_up_depth = 0;
            picture.set_phase(Phase::Armed);
            continue;
        }
        let text = match stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, %turn, "stt failed (model kept loaded)");
                picture.detail = Some(format!("stt: {e}"));
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        tracing::debug!(%turn, "stt kept loaded for next turn");
        picture.heard = Some(text.clone());
        picture.publish();
        tracing::info!(%turn, %text, "heard");

        // Host guard: skip agent on empty / whitespace / junk transcripts.
        if !transcript_usable(&text) {
            tracing::warn!(
                %turn,
                %text,
                alnum = text.chars().filter(|c| c.is_alphanumeric()).count(),
                "skipping empty/junk transcript — not calling agent"
            );
            picture.detail = Some("didn't catch that".into());
            // If we were in a follow-up, one soft retry is enough; then re-arm.
            if matches!(capture_kind, CaptureKind::AwaitReply) && follow_up_depth < MAX_FOLLOW_UPS {
                await_reply = true;
            } else {
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
            }
            continue;
        }

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        ) {
            go_off(&mut picture, &audio, &store, &mut active_session);
            continue;
        }

        // Think
        picture.set_phase(Phase::Thinking);
        let outcome = match agent.run_turn(&text) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, %turn, "agent failed");
                picture.detail = Some(format!("agent: {e}"));
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };

        let (reply, expect_reply) = match outcome {
            AgentOutcome::Speak { text, expect_reply } if !text.trim().is_empty() => {
                (text, expect_reply)
            }
            AgentOutcome::Speak { .. } | AgentOutcome::Silent => {
                tracing::warn!(%turn, "agent produced no speech");
                picture.detail = Some("empty agent reply".into());
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };

        if let Some(ref sid) = active_session {
            if let Err(e) = store.append_user_assistant(sid, &text, &reply) {
                tracing::warn!(
                    error = %e,
                    session_id = %sid,
                    %turn,
                    "session append_user_assistant failed"
                );
            }
        }

        picture.said = Some(reply.clone());
        picture.publish();
        tracing::info!(%turn, %reply, expect_reply, "said");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        ) {
            go_off(&mut picture, &audio, &store, &mut active_session);
            continue;
        }

        // Talk
        picture.set_phase(Phase::Talking);
        if let Err(e) = tts.load() {
            tracing::error!(error = %e, %turn, "tts load failed");
            picture.detail = Some(format!("tts load: {e}"));
            follow_up_depth = 0;
            picture.set_phase(Phase::Armed);
            continue;
        }
        let pcm = match tts.synthesize(&reply) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, %turn, "tts failed");
                picture.detail = Some(format!("tts: {e}"));
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };

        while output_events.try_recv().is_ok() {}
        audio.play(pcm);
        wait_playback_or_stop(
            &mut output_events,
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut picture,
        );

        if !running {
            go_off(&mut picture, &audio, &store, &mut active_session);
            continue;
        }

        // Freeform follow-up: any speakable answer (not yes/no only). Cap chain depth.
        if expect_reply && follow_up_depth < MAX_FOLLOW_UPS {
            follow_up_depth = follow_up_depth.saturating_add(1);
            await_reply = true;
            tracing::info!(
                %turn,
                depth = follow_up_depth,
                "will await freeform reply after speak"
            );
        } else {
            if expect_reply {
                tracing::info!(%turn, "expect_reply but follow-up cap reached — re-arming");
            }
            follow_up_depth = 0;
            await_reply = false;
        }

        tracing::info!(%turn, "turn complete");
    }
}

/// Start or resume a voice session and seed the agent context.
///
/// Soft-fails on store I/O — voice loop continues without persistence.
fn begin_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    agent: &mut AgentEngine,
    system_prompt: &str,
) {
    *active_session = None;
    let previous = match store.current_id() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "session current_id failed");
            None
        }
    };

    match store.resume_or_create() {
        Ok(meta) => {
            let resumed = previous.as_ref() == Some(&meta.id);
            if resumed {
                tracing::info!(session_id = %meta.id, "session resumed");
                match store.load_transcript(&meta.id) {
                    Ok(records) => {
                        let wire: Vec<(String, serde_json::Value)> = records
                            .into_iter()
                            .map(|r| (r.role, r.content))
                            .collect();
                        let history = Context::messages_from_transcript(&wire);
                        agent.load_session_history(system_prompt, history);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            session_id = %meta.id,
                            "failed to load session transcript; resetting conversation"
                        );
                        agent.reset_conversation(system_prompt);
                    }
                }
            } else {
                tracing::info!(session_id = %meta.id, "session created");
                agent.reset_conversation(system_prompt);
            }
            *active_session = Some(meta.id);
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "session resume_or_create failed; continuing without persistence"
            );
            agent.reset_conversation(system_prompt);
        }
    }
}

/// Soft-fail end of the current session (Stop / go_off / shutdown).
fn end_session(store: &SessionStore, active_session: &mut Option<SessionId>) {
    match store.end_current() {
        Ok(Some(meta)) => {
            tracing::info!(session_id = %meta.id, "session ended");
        }
        Ok(None) => {
            if let Some(ref sid) = active_session {
                tracing::debug!(session_id = %sid, "session end_current: no current pointer");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "session end_current failed");
        }
    }
    *active_session = None;
}

fn go_off(
    picture: &mut Picture,
    audio: &AudioService,
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
) {
    end_session(store, active_session);
    audio.stop();
    picture.engine = EngineState::Off;
    picture.turn = None;
    picture.set_phase(Phase::Off);
}

/// True when STT text is worth sending to the agent.
///
/// Rejects empty/whitespace and transcripts with fewer than 2 alphanumeric
/// characters (noise, partial wake, accidental clicks).
fn transcript_usable(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
}

/// Drain host commands; apply device switches immediately. Returns false if stopped.
fn poll_running(
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
) -> bool {
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *running = false;
                return false;
            }
            Ok(EngineCommand::Start) => *running = true,
            Ok(EngineCommand::SwitchInput { device_id }) => {
                apply_input_switch(audio, picture, &device_id);
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                apply_output_switch(audio, output_events, picture, &device_id);
            }
            Err(mpsc::TryRecvError::Empty) => return *running,
            Err(mpsc::TryRecvError::Disconnected) => {
                *running = false;
                return false;
            }
        }
    }
}

fn wait_playback_or_stop(
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    picture: &mut Picture,
) {
    loop {
        if !poll_running(cmd_rx, running, audio, output_events, picture) {
            audio.stop();
            return;
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(OutputEvent::Drained) => return,
            Ok(OutputEvent::Cleared) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn apply_input_switch(audio: &mut AudioService, picture: &mut Picture, device_id: &str) {
    match find_input(device_id) {
        Some(info) => match audio.switch_input(&info.id) {
            Ok(()) => {
                tracing::info!(name = %info.name, "switched input device");
                picture.mic = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "input switch failed");
                picture.detail = Some(format!("mic switch failed: {e}"));
                picture.mic.ok = false;
                picture.publish();
            }
        },
        None => {
            tracing::warn!(%device_id, "unknown input device");
            picture.detail = Some(format!("unknown microphone id"));
            picture.publish();
        }
    }
}

fn apply_output_switch(
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
    device_id: &str,
) {
    match find_output(device_id) {
        Some(info) => match audio.switch_output(&info.id) {
            Ok(()) => {
                tracing::info!(name = %info.name, "switched output device");
                // Output pipeline rebuilds its event channel — resubscribe.
                *output_events = audio.subscribe_output();
                picture.speaker = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "output switch failed");
                picture.detail = Some(format!("speaker switch failed: {e}"));
                picture.speaker.ok = false;
                picture.publish();
            }
        },
        None => {
            tracing::warn!(%device_id, "unknown output device");
            picture.detail = Some(format!("unknown speaker id"));
            picture.publish();
        }
    }
}

// ── Optional null backends when features are off ─────────────────────────────

#[cfg(not(feature = "stt-parakeet"))]
struct NullStt;

#[cfg(not(feature = "stt-parakeet"))]
impl SpeechToText for NullStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::error::Result<String> {
        Err(boris_core::error::Error::Other(
            "stt-parakeet feature disabled".into(),
        ))
    }
}

#[cfg(not(feature = "tts-supertone"))]
struct NullTts;

#[cfg(not(feature = "tts-supertone"))]
impl TextToSpeech for NullTts {
    fn synthesize(&mut self, _: &str) -> boris_core::error::Result<boris_core::AudioBuffer> {
        Err(boris_core::error::Error::Other(
            "tts-supertone feature disabled".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_off_default() {
        let s = StatusPicture::off();
        assert_eq!(s.engine, EngineState::Off);
        assert_eq!(s.phase, Phase::Off);
    }

    #[test]
    fn engine_command_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<EngineCommand>();
        assert_send::<EngineHandle>();
    }

    #[test]
    fn transcript_usable_rejects_empty_and_junk() {
        assert!(!transcript_usable(""));
        assert!(!transcript_usable("   \t\n"));
        assert!(!transcript_usable("a"));
        assert!(!transcript_usable("!"));
        assert!(!transcript_usable("."));
        assert!(!transcript_usable("a "));
        assert!(transcript_usable("hi"));
        assert!(transcript_usable("ok"));
        assert!(transcript_usable("hello world"));
        assert!(transcript_usable("  yo  "));
    }
}
