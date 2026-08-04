//! Voice engine for desktop.
//!
//! One engine thread owns the turn loop. STT/TTS load one step ahead on short
//! helper threads (STT while capturing, TTS while the agent thinks) so the UI
//! never shows "loading model" chrome — only real phases.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::context::Context;
use boris_agent::session::store::SessionStore;
use boris_agent::session::types::SessionId;
use boris_agent::{Agent, AgentOutcome, OpenRouterClient, SandboxConfig};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::types::ArcAudioBuffer;
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
/// Multi-step voice chores need more than a couple of back-and-forths.
const MAX_FOLLOW_UPS: u32 = 24;

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
    activity: Option<String>,
    context_used: Option<u32>,
    context_limit: Option<u32>,
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
            activity: self.activity.clone(),
            context_used: self.context_used,
            context_limit: self.context_limit,
        });
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.publish();
    }

    fn clear_activity(&mut self) {
        if self.activity.take().is_some() {
            self.publish();
        }
    }

    fn update_context_from_chars(&mut self, approx_chars: usize) {
        // Rough token estimate (chars/4), for the overlay meter only.
        let used = (approx_chars as u32 / 4).max(1);
        self.context_used = Some(used);
        self.context_limit = Some(crate::status::DEFAULT_CONTEXT_LIMIT_TOKENS);
        self.publish();
    }
}

fn run(
    config: PipelineConfig,
    cmd_rx: Receiver<EngineCommand>,
    status_tx: Sender<StatusPicture>,
) -> Result<(), String> {
    tracing::info!("engine thread entered run()");
    crate::diagnostics::log_environment("engine_run");
    crate::diagnostics::log_writable_check("boris_home", paths::boris_home());
    crate::diagnostics::log_writable_check("sessions", paths::sessions_dir());
    crate::diagnostics::log_writable_check("logs", paths::logs_dir());

    tracing::info!("init_onnx_runtime…");
    init_onnx_runtime();
    tracing::info!("init_onnx_runtime done");

    tracing::info!(
        play_source_rate = config.play_source_rate,
        wake_bytes = config.wakeword_model.len(),
        stt = %config.stt_model_dir.display(),
        tts = %config.tts_model_dir.display(),
        voices = %config.tts_voice_dir.display(),
        voice_id = %config.tts_voice_id,
        openrouter_model = ?config.openrouter_model,
        has_api_key = !config.openrouter_api_key.trim().is_empty(),
        "pipeline config (key redacted)"
    );

    tracing::info!("opening AudioService (default mic + speaker)…");
    let mut audio = match AudioService::with_source_rate(config.play_source_rate) {
        Ok(audio) => {
            tracing::info!("AudioService ready");
            audio
        }
        Err(e) => {
            tracing::error!(error = %e, "AudioService::with_source_rate FAILED");
            crate::diagnostics::log_environment("audio_init_failed");
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
                activity: None,
                context_used: None,
                context_limit: None,
            });
            return Err(format!("audio init failed: {e}"));
        }
    };
    let mic = audio.subscribe_input(Some(MIC_QUEUE));
    let mut output_events = audio.subscribe_output();
    tracing::info!(mic_queue = MIC_QUEUE, "subscribed to mic + output events");

    tracing::info!(
        wake_bytes = config.wakeword_model.len(),
        sample_rate = boris_audio::AUDIO_TARGET_RATE,
        "loading LivekitWakeWord (ORT sessions)…"
    );
    let mut wake = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LivekitWakeWord::new(
            "boris",
            &config.wakeword_model,
            boris_audio::AUDIO_TARGET_RATE,
        )
    })) {
        Ok(w) => {
            tracing::info!("LivekitWakeWord loaded");
            w
        }
        Err(payload) => {
            let msg = panic_payload_str(&payload);
            tracing::error!(
                error = %msg,
                wake_bytes = config.wakeword_model.len(),
                "LivekitWakeWord::new PANICKED — often missing onnxruntime.dll / DirectML.dll beside the exe"
            );
            crate::diagnostics::log_environment("wakeword_panic");
            let detail = format!("wakeword init panic: {msg}");
            let _ = status_tx.send(StatusPicture {
                engine: EngineState::Fault,
                phase: Phase::Off,
                detail: Some(detail.clone()),
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
                activity: None,
                context_used: None,
                context_limit: None,
            });
            return Err(detail);
        }
    };
    let mut vad = WebRtcVad::new();
    tracing::info!("WebRtcVad ready");

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

    // Long-lived Tokio runtime for the async agent plane (LLM + tools).
    // Voice capture / STT / TTS stay on this sync engine thread.
    let agent_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("boris-agent")
        .build()
        .expect("failed to build Tokio runtime for agent");

    tracing::info!("building OpenRouter client + Agent…");
    let client = OpenRouterClient::new(config.openrouter_api_key, config.openrouter_model);
    let mut agent = Agent::new(Box::new(client), &config.system_prompt);
    if let Err(e) = paths::ensure_agent_dirs() {
        tracing::warn!(error = %e, "ensure agent sandbox/audit dirs failed");
    }

    let preset = config.capability_preset;
    let mut sandbox = SandboxConfig::for_desktop_mvp(paths::boris_home());
    preset.apply_to_sandbox(&mut sandbox);
    agent.configure_runtime(sandbox, Some(paths::audit_path()));

    // Core + (optional) power tools filtered by capability preset + personal context.
    let power = preset.wants_power_tools();
    boris_agent::tools::register_builtin_tools_with_preset(
        &mut agent,
        boris_agent::tools::BuiltinToolPaths {
            notes_path: paths::notes_path(),
            profile_path: paths::profile_path(),
            sandbox_root: paths::sandbox_dir(),
            data_roots: vec![paths::memory_dir(), paths::sessions_dir()],
            allow_read: boris_agent::default_user_read_roots(),
            allow_write: vec![],
            boris_home: paths::boris_home(),
        },
        true,
        power,
        preset,
    );

    // Skills: install defaults into ~/.boris/skills if missing, then enable catalog + tools.
    match boris_agent::ensure_default_skills(&paths::boris_home()) {
        Ok(written) if !written.is_empty() => {
            tracing::info!(count = written.len(), "installed default skill playbooks");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "ensure default skills failed"),
    }
    let cwd = std::env::current_dir().ok();
    let loaded = boris_agent::load_skills(cwd.as_deref(), &paths::boris_home(), &[], true);
    let skill_count = loaded.skills.len();
    agent.enable_skills(loaded);

    if config.long_term_memory {
        match agent.enable_long_term_memory(paths::memory_dir()) {
            Ok(_) => tracing::info!(
                memory_md = %paths::memory_md_path().display(),
                "long-term markdown memory enabled"
            ),
            Err(e) => tracing::warn!(error = %e, "long-term memory enable failed"),
        }
    }

    tracing::info!(
        notes = %paths::notes_path().display(),
        profile = %paths::profile_path().display(),
        sandbox = %paths::sandbox_dir().display(),
        audit = %paths::audit_path().display(),
        skills = skill_count,
        skills_dir = %paths::skills_dir().display(),
        capability = preset.as_str(),
        long_term_memory = config.long_term_memory,
        "builtin tools + skills + memory + tool runtime registered"
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
        activity: None,
        context_used: None,
        context_limit: Some(crate::status::DEFAULT_CONTEXT_LIMIT_TOKENS),
        status_tx,
    };
    picture.publish();
    tracing::info!("engine idle (Off) — waiting for Start command");

    let mut running = false;
    let mut next_turn: u64 = 1;
    // When true, next iteration skips wake and freeform-listens for a reply.
    let mut await_reply = false;
    let mut follow_up_depth: u32 = 0;

    loop {
        // ── Off: wait for Start ─────────────────────────────────────────────
        if !running {
            await_reply = false;
            follow_up_depth = 0;
            match cmd_rx.recv() {
                Ok(EngineCommand::Start) => {
                    tracing::info!("EngineCommand::Start received");
                    running = true;
                    picture.heard = None;
                    picture.said = None;
                    picture.turn = None;
                    picture.detail = None;

                    // STT/TTS stay unloaded until a turn needs them (preloaded one
                    // step ahead during capture / agent — never kept for Armed idle).
                    begin_session(
                        &store,
                        &mut active_session,
                        &mut agent,
                        &config.system_prompt,
                    );
                    picture.engine = EngineState::On;
                    picture.set_phase(Phase::Armed);
                    tracing::info!(
                        "engine started — Armed, listening for wake (STT/TTS on-demand)"
                    );
                }
                Ok(EngineCommand::Stop) => continue,
                Ok(EngineCommand::Shutdown) | Err(_) => {
                    end_session(&store, &mut active_session);
                    release_voice_models(stt.as_mut(), tts.as_mut(), "shutdown");
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
        // Keep last `heard` + `said` while idle so Conversation shows the full
        // last turn (not just Boris). Clear both only when a new utterance starts.
        let capture_kind = if await_reply {
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
                        apply_output_switch(
                            &mut audio,
                            &mut output_events,
                            &mut picture,
                            &device_id,
                        );
                        continue;
                    }
                    HearBreak::Stopped if !running => {
                        go_off(
                            &mut picture,
                            &audio,
                            &store,
                            &mut active_session,
                            stt.as_mut(),
                            tts.as_mut(),
                        );
                        continue;
                    }
                    HearBreak::Stopped => {
                        await_reply = false;
                        follow_up_depth = 0;
                        continue;
                    }
                    HearBreak::Disconnected => {
                        end_session(&store, &mut active_session);
                        release_voice_models(stt.as_mut(), tts.as_mut(), "disconnected");
                        picture.set_phase(Phase::Off);
                        return Ok(());
                    }
                }
            }
            if !running {
                go_off(
                    &mut picture,
                    &audio,
                    &store,
                    &mut active_session,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                continue;
            }
            // Consumed for this entry; may re-arm after this turn if Boris asks again.
            await_reply = false;
            // New freeform utterance — drop previous lines now that we're recording.
            picture.said = None;
            picture.heard = None;
            CaptureKind::AwaitReply
        } else {
            follow_up_depth = 0;
            // Soft landing into Ready: keep last turn text so captions / Conversation
            // don't hard-cut when Speaking ends — clear only after the next wake.
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
                    go_off(
                        &mut picture,
                        &audio,
                        &store,
                        &mut active_session,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    continue;
                }
                Err(HearBreak::Stopped) => continue,
                Err(HearBreak::Disconnected) => {
                    end_session(&store, &mut active_session);
                    release_voice_models(stt.as_mut(), tts.as_mut(), "disconnected");
                    picture.set_phase(Phase::Off);
                    return Ok(());
                }
            }

            if !running {
                go_off(
                    &mut picture,
                    &audio,
                    &store,
                    &mut active_session,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                continue;
            }
            // New user turn — drop previous line now that we're listening again.
            picture.said = None;
            picture.heard = None;
            CaptureKind::AfterWake
        };

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId(next_turn);
        next_turn = next_turn.saturating_add(1);
        picture.turn = Some(turn);
        // Hearing only while the mic is actually recording (not during STT).
        // Preload STT in parallel — should be ready by the time capture ends.
        picture.set_phase(Phase::Hearing);
        tracing::info!(%turn, ?capture_kind, "turn begin — hearing (+ STT preload)");

        let stt_job = spawn_stt_load(stt);
        let capture = hear::capture_utterance(&mic, &mut vad, &cmd_rx, &mut running, capture_kind);
        let (stt_owned, stt_load) = join_stt_load(stt_job);
        stt = stt_owned;

        let clip = match capture {
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
                go_off(
                    &mut picture,
                    &audio,
                    &store,
                    &mut active_session,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                continue;
            }
            Err(HearBreak::Stopped) => {
                follow_up_depth = 0;
                continue;
            }
            Err(HearBreak::Disconnected) => {
                end_session(&store, &mut active_session);
                release_voice_models(stt.as_mut(), tts.as_mut(), "disconnected");
                return Ok(());
            }
        };

        if !running {
            go_off(
                &mut picture,
                &audio,
                &store,
                &mut active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            continue;
        }

        if let Err(e) = stt_load {
            tracing::error!(error = %e, %turn, "stt load failed");
            crate::diagnostics::log_model_load_failure("parakeet", &config.stt_model_dir, &e);
            let _ = stt.unload();
            picture.detail = Some(format!("stt load: {e}"));
            follow_up_depth = 0;
            picture.set_phase(Phase::Armed);
            continue;
        }

        // Leave Hearing as soon as the mic stops — STT is "Reading", not listening.
        picture.set_phase(Phase::Reading);
        let stt_t = std::time::Instant::now();
        let text = match stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, %turn, "stt failed");
                let _ = stt.unload();
                picture.detail = Some(format!("stt: {e}"));
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        // Free ~600MB before agent; TTS will preload while the agent runs.
        unload_stt(stt.as_mut(), turn);
        let stt_ms = stt_t.elapsed().as_millis() as u64;
        tracing::info!(
            %turn,
            stt_ms,
            clip_samples = clip.len(),
            clip_ms = (clip.len() as u64 * 1000) / 16_000,
            "stt done"
        );
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
        )
        .still_running()
        {
            go_off(
                &mut picture,
                &audio,
                &store,
                &mut active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            continue;
        }

        // Think + preload TTS in parallel (one step ahead of synthesize).
        picture.activity = Some("thinking…".into());
        picture.set_phase(Phase::Thinking);
        // Seed context meter from current conversation size.
        let approx_in = agent
            .export_messages()
            .iter()
            .fold(0usize, |acc, m| acc + m.content.to_string().len());
        picture.update_context_from_chars(approx_in + text.len());

        let agent_t = std::time::Instant::now();
        let tts_job = spawn_tts_load(tts);
        agent.set_turn_id(Some(turn.to_string()));
        if let Some(ref sid) = active_session {
            agent.set_session_id(Some(sid.to_string()));
        }

        // Live tool activity → overlay (subscribe emits on this thread inside block_on).
        let activity_bridge = std::sync::Arc::new(std::sync::Mutex::new(picture.status_tx.clone()));
        let activity_snap = std::sync::Arc::new(std::sync::Mutex::new((
            picture.engine,
            picture.phase,
            picture.detail.clone(),
            picture.heard.clone(),
            picture.said.clone(),
            picture.mic.clone(),
            picture.speaker.clone(),
            picture.turn,
            picture.context_used,
            picture.context_limit,
        )));
        let snap_w = activity_snap.clone();
        let tx_w = activity_bridge.clone();
        let _unsub = agent.subscribe(move |ev| {
            use boris_agent::AgentEvent;
            let label = match ev {
                AgentEvent::ToolExecutionStart { tool_name, .. } => {
                    Some(format!("tool · {tool_name}"))
                }
                AgentEvent::ToolExecutionEnd { tool_name, ok, .. } => Some(if *ok {
                    format!("done · {tool_name}")
                } else {
                    format!("fail · {tool_name}")
                }),
                AgentEvent::TurnStart { round } if *round > 0 => {
                    Some(format!("thinking · round {}", round + 1))
                }
                AgentEvent::NeedsConfirmation { pending } => {
                    Some(format!("confirm · {}", pending.name))
                }
                _ => None,
            };
            let Some(label) = label else { return };
            let Ok(g) = snap_w.lock() else { return };
            let (
                engine,
                phase,
                detail,
                heard,
                said,
                mic,
                speaker,
                turn_id,
                context_used,
                context_limit,
            ) = g.clone();
            drop(g);
            if let Ok(tx) = tx_w.lock() {
                let _ = tx.send(StatusPicture {
                    engine,
                    phase,
                    detail,
                    heard,
                    said,
                    mic,
                    speaker,
                    turn: turn_id.map(|t| t.to_string()),
                    activity: Some(label),
                    context_used,
                    context_limit,
                });
            }
        });

        let outcome = agent_rt.block_on(agent.prompt_with_report(&text));
        _unsub(); // drop live tool listener
        let (tts_owned, tts_load) = join_tts_load(tts_job);
        tts = tts_owned;

        let (outcome, report) = match outcome {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(error = %e, %turn, "agent failed");
                agent.cancel_pending();
                picture.detail = None;
                picture.clear_activity();
                // Recoverable spoken line instead of silent Ready.
                let recovery =
                    "I glitched mid-task. Wake me and say continue if you want me to finish.";
                picture.said = Some(recovery.into());
                picture.publish();
                if tts.load().is_ok() {
                    if let Ok(pcm) = tts.synthesize(recovery) {
                        while output_events.try_recv().is_ok() {}
                        audio.play(pcm);
                        let _ = wait_playback_started(
                            &mut output_events,
                            &cmd_rx,
                            &mut running,
                            &mut audio,
                            &mut picture,
                        );
                        wait_playback_or_stop(
                            &mut output_events,
                            &cmd_rx,
                            &mut running,
                            &mut audio,
                            &mut picture,
                        );
                    }
                    let _ = tts.unload();
                }
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        picture.activity = if report.tools_used.is_empty() {
            None
        } else {
            Some(format!("{} tools", report.tools_used.len()))
        };
        picture.update_context_from_chars(report.approx_chars_in);

        // Resolve HITL confirmations (voice yes/no) before final speech.
        let outcome = match resolve_agent_outcome(
            outcome,
            &mut agent,
            &agent_rt,
            &mut tts,
            &mut stt,
            &mic,
            &mut vad,
            &mut audio,
            &mut output_events,
            &cmd_rx,
            &mut running,
            &mut picture,
            &store,
            &mut active_session,
            turn,
        ) {
            OutcomeResolve::Stopped => continue,
            OutcomeResolve::ReArm => {
                follow_up_depth = 0;
                continue;
            }
            OutcomeResolve::Done(o) => o,
        };

        let agent_ms = agent_t.elapsed().as_millis() as u64;
        tracing::info!(%turn, agent_ms, "agent done");

        let (reply, expect_reply) = match outcome {
            AgentOutcome::Speak { text, expect_reply } if !text.trim().is_empty() => {
                (text, expect_reply)
            }
            AgentOutcome::Silent if !report.tools_used.is_empty() => {
                // Tools ran but model returned empty — still tell the user.
                (
                    "I ran tools but lost the spoken wrap-up. Say continue and I'll pick up."
                        .to_string(),
                    true, // keep freeform listen so they can say "continue"
                )
            }
            AgentOutcome::Speak { .. }
            | AgentOutcome::Silent
            | AgentOutcome::NeedsConfirmation { .. } => {
                tracing::warn!(%turn, "agent produced no speech");
                let _ = tts.unload();
                picture.detail = None;
                picture.said = Some("I didn't get a reply out. Wake me and try again.".into());
                picture.publish();
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

        // Show reply text while still "Thinking" — TTS synth is NOT speaking yet.
        picture.said = Some(reply.clone());
        picture.publish();
        tracing::info!(%turn, %reply, expect_reply, "said (synth next)");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        )
        .still_running()
        {
            go_off(
                &mut picture,
                &audio,
                &store,
                &mut active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            continue;
        }

        if let Err(e) = tts_load {
            tracing::error!(error = %e, %turn, "tts load failed");
            crate::diagnostics::log_model_load_failure("supertone", &config.tts_model_dir, &e);
            let _ = tts.unload();
            picture.detail = Some(format!("tts load: {e}"));
            follow_up_depth = 0;
            picture.set_phase(Phase::Armed);
            continue;
        }

        // Synthesize under Thinking so UI doesn't say "Speaking" with silence.
        let tts_t = std::time::Instant::now();
        let pcm = match tts.synthesize(&reply) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, %turn, "tts failed");
                let _ = tts.unload();
                picture.detail = Some(format!("tts: {e}"));
                follow_up_depth = 0;
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        // Free TTS during playback; optionally preload STT for freeform follow-up.
        unload_tts(tts.as_mut(), turn);
        let tts_ms = tts_t.elapsed().as_millis() as u64;
        let play_samples = pcm.len();
        tracing::info!(%turn, tts_ms, play_samples, "tts synth done");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        )
        .still_running()
        {
            go_off(
                &mut picture,
                &audio,
                &store,
                &mut active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            continue;
        }

        // Decide follow-up before playback so we can preload STT while speaking.
        let will_await = expect_reply && follow_up_depth < MAX_FOLLOW_UPS;
        // Move STT into a load job (or keep it in the Option) so the compiler
        // always sees a single reclaim path after playback.
        let mut stt_slot: Option<SttBox> = Some(stt);
        let mut stt_follow_job = if will_await {
            Some(spawn_stt_load(stt_slot.take().expect("stt")))
        } else {
            None
        };

        // Queue audio, then flip UI to Talking only when playback has started.
        // Speaker switch mid-play rebuilds the output pipeline and aborts the job —
        // we must not hang waiting for Drained on a dead channel (stuck Talking).
        let play_t = std::time::Instant::now();
        while output_events.try_recv().is_ok() {}
        audio.play(pcm);
        match wait_playback_started(
            &mut output_events,
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut picture,
        ) {
            PlaybackWait::Stopped => {
                stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);
                go_off(
                    &mut picture,
                    &audio,
                    &store,
                    &mut active_session,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                continue;
            }
            PlaybackWait::Aborted => {
                tracing::info!(
                    %turn,
                    "playback aborted (speaker switch) before/during start — skipping Talking"
                );
            }
            PlaybackWait::Finished => {
                picture.set_phase(Phase::Talking);
                tracing::info!(
                    %turn,
                    queue_ms = play_t.elapsed().as_millis() as u64,
                    "playback started — UI Talking"
                );
                wait_playback_or_stop(
                    &mut output_events,
                    &cmd_rx,
                    &mut running,
                    &mut audio,
                    &mut picture,
                );
            }
        }
        let play_ms = play_t.elapsed().as_millis() as u64;

        stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);

        if !running {
            go_off(
                &mut picture,
                &audio,
                &store,
                &mut active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            continue;
        }

        // Freeform follow-up: any speakable answer (not yes/no only). Cap chain depth.
        if will_await {
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
            // No follow-up — make sure heavy models are gone for Armed idle.
            unload_stt(stt.as_mut(), turn);
            unload_tts(tts.as_mut(), turn);
        }

        tracing::info!(
            %turn,
            stt_ms,
            agent_ms,
            tts_ms,
            play_ms,
            total_post_capture_ms = stt_ms + agent_ms + tts_ms + play_ms,
            "turn complete (latency breakdown)"
        );
    }
}

enum OutcomeResolve {
    Done(AgentOutcome),
    ReArm,
    Stopped,
}

/// Drive NeedsConfirmation → speak → freeform yes/no → resume until Speak/Silent.
fn resolve_agent_outcome(
    mut outcome: AgentOutcome,
    agent: &mut Agent,
    agent_rt: &tokio::runtime::Runtime,
    tts: &mut TtsBox,
    stt: &mut SttBox,
    mic: &crossbeam_channel::Receiver<ArcAudioBuffer>,
    vad: &mut impl boris_sense::Vad,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    picture: &mut Picture,
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    turn: TurnId,
) -> OutcomeResolve {
    // Cap nested confirms (also enforced in agent policy).
    for _ in 0..4 {
        let AgentOutcome::NeedsConfirmation {
            text: prompt,
            pending,
        } = outcome
        else {
            return OutcomeResolve::Done(outcome);
        };

        tracing::info!(
            %turn,
            tool = %pending.name,
            pending_id = %pending.id,
            "agent needs confirmation"
        );
        // Never put confirm text in `detail` (overlay treats detail as error).
        picture.detail = None;
        picture.activity = Some(format!("confirm · {}", pending.name));
        picture.said = Some(prompt.clone());
        picture.publish();

        // Speak the confirm prompt.
        if let Err(e) = tts.load() {
            tracing::error!(error = %e, "tts load failed for confirm");
            agent.cancel_pending();
            picture.detail = Some(format!("tts: {e}"));
            picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }
        let pcm = match tts.synthesize(&prompt) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "tts synth failed for confirm");
                agent.cancel_pending();
                let _ = tts.unload();
                picture.detail = Some(format!("tts: {e}"));
                picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
        while output_events.try_recv().is_ok() {}
        audio.play(pcm);
        match wait_playback_started(output_events, cmd_rx, running, audio, picture) {
            PlaybackWait::Stopped => {
                agent.cancel_pending();
                go_off(
                    picture,
                    audio,
                    store,
                    active_session,
                    stt.as_mut(),
                    tts.as_mut(),
                );
                return OutcomeResolve::Stopped;
            }
            PlaybackWait::Aborted => {}
            PlaybackWait::Finished => {
                picture.set_phase(Phase::Talking);
                wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
            }
        }
        if !*running {
            agent.cancel_pending();
            go_off(
                picture,
                audio,
                store,
                active_session,
                stt.as_mut(),
                tts.as_mut(),
            );
            return OutcomeResolve::Stopped;
        }

        // Listen for yes/no with longer post-TTS settle + confirm-specific VAD.
        picture.set_phase(Phase::AwaitingConfirm);
        if let Err(e) = hear::settle_after_confirm(mic, cmd_rx, running) {
            agent.cancel_pending();
            picture.clear_activity();
            return match e {
                HearBreak::Stopped if !*running => {
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    OutcomeResolve::Stopped
                }
                HearBreak::Disconnected => {
                    end_session(store, active_session);
                    release_voice_models(stt.as_mut(), tts.as_mut(), "disconnected");
                    picture.set_phase(Phase::Off);
                    OutcomeResolve::Stopped
                }
                _ => {
                    picture.set_phase(Phase::Armed);
                    OutcomeResolve::ReArm
                }
            };
        }

        if let Err(e) = stt.load() {
            tracing::error!(error = %e, "stt load failed for confirm");
            agent.cancel_pending();
            picture.detail = Some(format!("stt: {e}"));
            picture.clear_activity();
            picture.set_phase(Phase::Armed);
            return OutcomeResolve::ReArm;
        }

        picture.set_phase(Phase::Hearing);
        let clip =
            match hear::capture_utterance(mic, vad, cmd_rx, running, CaptureKind::AwaitConfirm) {
                Ok(c) => c,
                Err(HearBreak::Stopped) if !*running => {
                    agent.cancel_pending();
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    return OutcomeResolve::Stopped;
                }
                Err(_) => {
                    // Silence: re-prompt once instead of silently rejecting.
                    tracing::info!(%turn, "confirm capture empty — re-prompt");
                    let reask = "I need a yes or no on that.";
                    picture.said = Some(reask.into());
                    picture.activity = Some("confirm · say yes or no".into());
                    picture.publish();
                    if let Ok(pcm) = tts.synthesize(reask) {
                        while output_events.try_recv().is_ok() {}
                        audio.play(pcm);
                        let _ =
                            wait_playback_started(output_events, cmd_rx, running, audio, picture);
                        wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
                    }
                    if !*running {
                        agent.cancel_pending();
                        go_off(
                            picture,
                            audio,
                            store,
                            active_session,
                            stt.as_mut(),
                            tts.as_mut(),
                        );
                        return OutcomeResolve::Stopped;
                    }
                    let _ = hear::settle_after_confirm(mic, cmd_rx, running);
                    picture.set_phase(Phase::Hearing);
                    match hear::capture_utterance(
                        mic,
                        vad,
                        cmd_rx,
                        running,
                        CaptureKind::AwaitConfirm,
                    ) {
                        Ok(c) => c,
                        Err(_) => {
                            tracing::info!(%turn, "confirm second capture failed — reject");
                            outcome = match agent_rt
                                .block_on(agent.resume_confirmation(&pending.id, false))
                            {
                                Ok(o) => o,
                                Err(e) => {
                                    tracing::error!(error = %e, "resume reject failed");
                                    agent.cancel_pending();
                                    picture.detail = Some(format!("agent: {e}"));
                                    picture.clear_activity();
                                    picture.set_phase(Phase::Armed);
                                    return OutcomeResolve::ReArm;
                                }
                            };
                            picture.activity = Some("thinking…".into());
                            picture.set_phase(Phase::Thinking);
                            continue;
                        }
                    }
                }
            };

        picture.set_phase(Phase::Reading);
        let heard = match stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "confirm STT failed");
                String::new()
            }
        };
        let _ = stt.unload();
        picture.heard = Some(heard.clone());
        picture.publish();

        let approved = match interpret_yes_no(&heard) {
            Some(v) => v,
            None => {
                tracing::info!(%turn, heard = %heard, "confirm answer ambiguous — re-ask once");
                let reask = "Was that a yes or a no?";
                picture.said = Some(reask.into());
                picture.activity = Some("confirm · yes or no".into());
                picture.publish();
                if let Ok(pcm) = tts.synthesize(reask) {
                    while output_events.try_recv().is_ok() {}
                    audio.play(pcm);
                    let _ = wait_playback_started(output_events, cmd_rx, running, audio, picture);
                    wait_playback_or_stop(output_events, cmd_rx, running, audio, picture);
                }
                if !*running {
                    agent.cancel_pending();
                    go_off(
                        picture,
                        audio,
                        store,
                        active_session,
                        stt.as_mut(),
                        tts.as_mut(),
                    );
                    return OutcomeResolve::Stopped;
                }
                picture.set_phase(Phase::AwaitingConfirm);
                let _ = hear::settle_after_confirm(mic, cmd_rx, running);
                let _ = stt.load();
                picture.set_phase(Phase::Hearing);
                let second =
                    hear::capture_utterance(mic, vad, cmd_rx, running, CaptureKind::AwaitConfirm)
                        .ok()
                        .and_then(|c| stt.transcribe(&c).ok())
                        .unwrap_or_default();
                let _ = stt.unload();
                picture.heard = Some(second.clone());
                picture.publish();
                interpret_yes_no(&second).unwrap_or(false)
            }
        };

        tracing::info!(%turn, approved, heard = %heard, "confirm decision");
        picture.activity = Some("thinking…".into());
        picture.set_phase(Phase::Thinking);
        picture.detail = None;
        outcome = match agent_rt.block_on(agent.resume_confirmation(&pending.id, approved)) {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "resume confirmation failed");
                agent.cancel_pending();
                picture.detail = Some(format!("agent: {e}"));
                picture.set_phase(Phase::Armed);
                return OutcomeResolve::ReArm;
            }
        };
    }

    tracing::warn!(%turn, "confirm loop cap — cancelling pending");
    agent.cancel_pending();
    picture.detail = Some("too many confirmations".into());
    picture.set_phase(Phase::Armed);
    OutcomeResolve::ReArm
}

/// Normalize STT: lowercase, strip most punctuation, collapse spaces.
fn normalize_confirm_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch.is_whitespace() || ch == '\'' {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' {
            out.push(' ');
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Interpret freeform STT as yes/no. `None` = ambiguous.
///
/// Accepts natural variants ("yeah go ahead", "nope cancel that", "yes.") not only
/// bare yes/no.
fn interpret_yes_no(text: &str) -> Option<bool> {
    let t = normalize_confirm_text(text);
    if t.is_empty() {
        return None;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let head: String = words.iter().take(5).cloned().collect::<Vec<_>>().join(" ");

    // Multi-word phrases first (order matters for "do not" / "go ahead").
    const YES_PHRASES: &[&str] = &[
        "go ahead",
        "go for it",
        "do it",
        "do that",
        "sounds good",
        "all right",
        "alright",
        "for sure",
        "why not",
        "yes please",
        "yeah sure",
        "yep sure",
        "ok go",
        "okay go",
    ];
    const NO_PHRASES: &[&str] = &[
        "do not",
        "don't",
        "no way",
        "no thanks",
        "not now",
        "hell no",
        "nope cancel",
        "cancel that",
        "stop that",
        "don't do",
        "do not do",
    ];

    for p in NO_PHRASES {
        if head == *p || head.starts_with(&format!("{p} ")) || t.contains(p) {
            return Some(false);
        }
    }
    for p in YES_PHRASES {
        if head == *p || head.starts_with(&format!("{p} ")) || t.contains(p) {
            return Some(true);
        }
    }

    const YES: &[&str] = &[
        "yes",
        "yeah",
        "yep",
        "yup",
        "sure",
        "ok",
        "okay",
        "please",
        "affirmative",
        "y",
        "yea",
        "confirmed",
        "confirm",
        "approve",
        "approved",
        "fine",
        "proceed",
        "continue",
        "absolutely",
        "definitely",
        "correct",
        "right",
        "true",
        "uh huh",
        "mhmm",
        "mm hmm",
    ];
    const NO: &[&str] = &[
        "no", "nope", "nah", "cancel", "stop", "never", "n", "negative", "decline", "abort",
        "refuse", "reject", "denied", "pass", "skip",
    ];

    // First-word / head match.
    for n in NO {
        if head == *n || head.starts_with(&format!("{n} ")) {
            return Some(false);
        }
    }
    for y in YES {
        if head == *y || head.starts_with(&format!("{y} ")) {
            return Some(true);
        }
    }

    // Any clear token in the utterance (handles "uh yes bro", "mm no thanks").
    let mut saw_yes = false;
    let mut saw_no = false;
    for w in &words {
        if matches!(
            *w,
            "yes" | "yeah" | "yep" | "yup" | "yea" | "sure" | "ok" | "okay" | "affirmative"
        ) {
            saw_yes = true;
        }
        if matches!(
            *w,
            "no" | "nope" | "nah" | "cancel" | "stop" | "never" | "negative" | "abort" | "decline"
        ) {
            saw_no = true;
        }
    }
    // Prefer deny if both appear ("yes no wait" → ambiguous → None, re-ask).
    match (saw_yes, saw_no) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Start or resume a voice session and seed the agent context.
///
/// Soft-fails on store I/O — voice loop continues without persistence.
fn begin_session(
    store: &SessionStore,
    active_session: &mut Option<SessionId>,
    agent: &mut Agent,
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
                        let wire: Vec<(String, serde_json::Value)> =
                            records.into_iter().map(|r| (r.role, r.content)).collect();
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
    stt: &mut dyn SpeechToText,
    tts: &mut dyn TextToSpeech,
) {
    end_session(store, active_session);
    audio.stop();
    release_voice_models(stt, tts, "engine stop");
    picture.engine = EngineState::Off;
    picture.turn = None;
    picture.set_phase(Phase::Off);
}

/// Drop STT + TTS weights from RAM. Safe if already unloaded.
fn release_voice_models(stt: &mut dyn SpeechToText, tts: &mut dyn TextToSpeech, reason: &str) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %reason, "stt unload failed");
    }
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %reason, "tts unload failed");
    }
    tracing::info!(%reason, "STT/TTS released (idle RAM)");
}

type SttBox = Box<dyn SpeechToText>;
type TtsBox = Box<dyn TextToSpeech>;

/// Load STT on a helper thread (overlaps with mic capture / playback).
fn spawn_stt_load(mut stt: SttBox) -> JoinHandle<(SttBox, Result<(), String>)> {
    thread::Builder::new()
        .name("boris-stt-load".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let r = stt.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "stt preload ready");
            }
            (stt, r)
        })
        .expect("spawn stt load thread")
}

fn join_stt_load(job: JoinHandle<(SttBox, Result<(), String>)>) -> (SttBox, Result<(), String>) {
    job.join().unwrap_or_else(|_| {
        tracing::error!("stt load thread panicked");
        // Recover with a no-op stub so the engine can still stop cleanly.
        // Real STT is lost only if the load thread panicked mid-flight.
        (
            Box::new(PanicLostStt),
            Err("stt load thread panicked".into()),
        )
    })
}

/// Reclaim STT after an optional follow-up preload job (or the idle slot).
fn reclaim_stt_slot(
    slot: &mut Option<SttBox>,
    job: &mut Option<JoinHandle<(SttBox, Result<(), String>)>>,
) -> SttBox {
    if let Some(j) = job.take() {
        let (stt, load_r) = join_stt_load(j);
        if let Err(e) = load_r {
            tracing::warn!(error = %e, "stt follow-up preload failed (will retry on next turn)");
        }
        return stt;
    }
    slot.take().expect("stt slot empty")
}

/// Load TTS on a helper thread (overlaps with agent thinking).
fn spawn_tts_load(mut tts: TtsBox) -> JoinHandle<(TtsBox, Result<(), String>)> {
    thread::Builder::new()
        .name("boris-tts-load".into())
        .spawn(move || {
            let t = std::time::Instant::now();
            let r = tts.load().map_err(|e| e.to_string());
            if r.is_ok() {
                tracing::info!(ms = t.elapsed().as_millis() as u64, "tts preload ready");
            }
            (tts, r)
        })
        .expect("spawn tts load thread")
}

fn join_tts_load(job: JoinHandle<(TtsBox, Result<(), String>)>) -> (TtsBox, Result<(), String>) {
    job.join().unwrap_or_else(|_| {
        tracing::error!("tts load thread panicked");
        (
            Box::new(PanicLostTts),
            Err("tts load thread panicked".into()),
        )
    })
}

fn unload_stt(stt: &mut dyn SpeechToText, turn: TurnId) {
    if let Err(e) = stt.unload() {
        tracing::warn!(error = %e, %turn, "stt unload failed");
    } else {
        tracing::debug!(%turn, "stt unloaded");
    }
}

fn unload_tts(tts: &mut dyn TextToSpeech, turn: TurnId) {
    if let Err(e) = tts.unload() {
        tracing::warn!(error = %e, %turn, "tts unload failed");
    } else {
        tracing::debug!(%turn, "tts unloaded");
    }
}

/// Placeholder if the STT load thread panics (should never happen in practice).
struct PanicLostStt;
impl SpeechToText for PanicLostStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::error::Result<String> {
        Err(boris_core::error::Error::Other(
            "STT model lost after load-thread panic".into(),
        ))
    }
}

struct PanicLostTts;
impl TextToSpeech for PanicLostTts {
    fn synthesize(&mut self, _: &str) -> boris_core::error::Result<boris_core::AudioBuffer> {
        Err(boris_core::error::Error::Other(
            "TTS model lost after load-thread panic".into(),
        ))
    }
}

/// True when STT text is worth sending to the agent.
///
/// Rejects empty/whitespace and transcripts with fewer than 2 alphanumeric
/// characters (noise, partial wake, accidental clicks).
fn transcript_usable(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
}

/// Result of draining host commands once.
#[derive(Debug, Clone, Copy)]
struct PollOutcome {
    /// Engine still on.
    running: bool,
    /// Output pipeline was rebuilt — any in-flight Play is gone and its
    /// Started/Drained events will never arrive on the new channel.
    output_rebuilt: bool,
}

impl PollOutcome {
    fn still_running(self) -> bool {
        self.running
    }
}

/// How a playback wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackWait {
    /// Natural finish (Started then later Drained), or Started observed.
    Finished,
    /// Speaker switched / Flush — do not keep waiting for dead events.
    Aborted,
    /// Host stop / disconnect — go Off.
    Stopped,
}

/// Drain host commands; apply device switches immediately.
fn poll_running(
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
) -> PollOutcome {
    let mut output_rebuilt = false;
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *running = false;
                return PollOutcome {
                    running: false,
                    output_rebuilt,
                };
            }
            Ok(EngineCommand::Start) => *running = true,
            Ok(EngineCommand::SwitchInput { device_id }) => {
                apply_input_switch(audio, picture, &device_id);
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                if apply_output_switch(audio, output_events, picture, &device_id) {
                    output_rebuilt = true;
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                return PollOutcome {
                    running: *running,
                    output_rebuilt,
                };
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                *running = false;
                return PollOutcome {
                    running: false,
                    output_rebuilt,
                };
            }
        }
    }
}

/// Wait until the output worker has resampled + queued samples (about to be audible).
fn wait_playback_started(
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    picture: &mut Picture,
) -> PlaybackWait {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let poll = poll_running(cmd_rx, running, audio, output_events, picture);
        if !poll.running {
            audio.stop();
            return PlaybackWait::Stopped;
        }
        if poll.output_rebuilt {
            tracing::info!("speaker switched before playback Started — aborting play wait");
            return PlaybackWait::Aborted;
        }
        if std::time::Instant::now() > deadline {
            tracing::warn!("playback Started timeout — flipping UI anyway");
            return if *running {
                PlaybackWait::Finished
            } else {
                PlaybackWait::Stopped
            };
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(OutputEvent::Started) => return PlaybackWait::Finished,
            // Short clips may drain before we observe Started if we missed it — treat as ok.
            Ok(OutputEvent::Drained) => return PlaybackWait::Finished,
            Ok(OutputEvent::Cleared) => return PlaybackWait::Aborted,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return PlaybackWait::Stopped;
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
        let poll = poll_running(cmd_rx, running, audio, output_events, picture);
        if !poll.running {
            audio.stop();
            return;
        }
        if poll.output_rebuilt {
            // Old pipeline (and its Drained event) is gone with the device rebuild.
            tracing::info!("speaker switched mid-playback — ending Talking wait");
            return;
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(OutputEvent::Started) => continue, // already speaking
            Ok(OutputEvent::Drained) => return,
            Ok(OutputEvent::Cleared) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn panic_payload_str(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".into()
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

/// Switch speaker. Returns `true` when the output pipeline was rebuilt
/// (in-flight playback and its event stream are gone).
fn apply_output_switch(
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
    device_id: &str,
) -> bool {
    match find_output(device_id) {
        Some(info) => match audio.switch_output(&info.id) {
            Ok(rebuilt) => {
                if rebuilt {
                    tracing::info!(name = %info.name, "switched output device");
                    // Pipeline rebuild drops in-flight Play + its event stream.
                    // Waiters must treat this as playback abort (`output_rebuilt`).
                    *output_events = audio.subscribe_output();
                } else {
                    tracing::debug!(name = %info.name, "output already selected");
                }
                picture.speaker = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
                rebuilt
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "output switch failed");
                picture.detail = Some(format!("speaker switch failed: {e}"));
                picture.speaker.ok = false;
                picture.publish();
                false
            }
        },
        None => {
            tracing::warn!(%device_id, "unknown output device");
            picture.detail = Some(format!("unknown speaker id"));
            picture.publish();
            false
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

    #[test]
    fn interpret_yes_no_variants() {
        assert_eq!(interpret_yes_no("yes"), Some(true));
        assert_eq!(interpret_yes_no("Yes."), Some(true));
        assert_eq!(interpret_yes_no("yeah go ahead"), Some(true));
        assert_eq!(interpret_yes_no("sure thing bro"), Some(true));
        assert_eq!(interpret_yes_no("ok do it"), Some(true));
        assert_eq!(interpret_yes_no("no"), Some(false));
        assert_eq!(interpret_yes_no("Nope!"), Some(false));
        assert_eq!(interpret_yes_no("nah cancel that"), Some(false));
        assert_eq!(interpret_yes_no("don't"), Some(false));
        assert_eq!(interpret_yes_no("uh maybe later"), None);
        assert_eq!(interpret_yes_no(""), None);
    }
}
