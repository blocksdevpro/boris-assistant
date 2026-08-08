//! Voice engine for desktop — sequential turns on one thread.
//!
//! # Module map (contributor navigation)
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`setup`] | One-time audio / wake / agent init |
//! | [`models`] | STT/TTS preload helpers (load-ahead threads) |
//! | [`confirm`] | Pure yes/no STT interpretation |
//! | [`outcome`] | HITL confirm path (speak → yes/no → resume) |
//! | [`session`] | Session begin/end + `go_off` |
//! | [`playback`] | Host-command poll + speaker wait |
//! | [`llm`] | OpenRouter model routing |
//! | [`device_switch`] | Mic / speaker switches |
//! | [`picture`] | Status publisher → UI |
//! | [`util`] | Small pure helpers |
//!
//! # Turn loop
//!
//! ```text
//! Start → Armed → (wake | await reply) → hear → read → think → talk → Armed → …
//! ```
//!
//! Wake scoring, VAD capture, STT, agent, and TTS are **called inline** on the
//! engine thread (or briefly block it). Status is pushed for the UI. Hosts send
//! [`EngineCommand`] via [`EngineHandle`].

mod confirm;
mod device_switch;
mod llm;
mod models;
mod outcome;
mod picture;
mod playback;
mod session;
mod setup;
mod util;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::session::types::SessionId;
use boris_agent::AgentOutcome;
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::TurnId;

use crate::config::PipelineConfig;
use crate::hear::{self, CaptureKind, HearBreak};
use crate::status::{EngineState, Phase, StatusPicture};

use device_switch::{apply_input_switch, apply_output_switch};
use models::{
    join_stt_load, join_tts_load, reclaim_stt_slot, release_voice_models, spawn_stt_load,
    spawn_tts_load, unload_stt, unload_tts, SttBox,
};
use outcome::{resolve_agent_outcome, OutcomeResolve};
use playback::{poll_running, wait_playback_or_stop, wait_playback_started, PlaybackWait};
use session::{agent_message_pairs, begin_session, end_session, go_off};
use setup::init_runtime;
use util::transcript_usable;

/// Mic fan-out queue. Must stay large enough that a brief engine stall during
/// capture (logging, status publish) never drops live speech frames.
pub(super) const MIC_QUEUE: usize = 256;

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

fn run(
    config: PipelineConfig,
    cmd_rx: Receiver<EngineCommand>,
    status_tx: Sender<StatusPicture>,
) -> Result<(), String> {
    let mut rt = init_runtime(config, status_tx)?;

    let mut running = false;
    let mut next_turn: u64 = 1;
    // When true, next iteration skips wake and freeform-listens for a reply.
    let mut await_reply = false;
    let mut follow_up_depth: u32 = 0;
    let mut active_session: Option<SessionId> = None;
    // How many agent messages are already on disk for `active_session`.
    let mut transcript_len: usize = 0;

    loop {
        // ── Off: wait for Start ─────────────────────────────────────────────
        if !running {
            await_reply = false;
            follow_up_depth = 0;
            match cmd_rx.recv() {
                Ok(EngineCommand::Start) => {
                    tracing::info!("EngineCommand::Start received");
                    running = true;
                    rt.picture.heard = None;
                    rt.picture.said = None;
                    rt.picture.turn = None;
                    rt.picture.detail = None;

                    // STT/TTS stay unloaded until a turn needs them (preloaded one
                    // step ahead during capture / agent — never kept for Armed idle).
                    begin_session(
                        &rt.store,
                        &mut active_session,
                        &mut transcript_len,
                        &mut rt.agent,
                        &rt.system_prompt,
                    );
                    rt.picture.engine = EngineState::On;
                    rt.picture.set_phase(Phase::Armed);
                    tracing::info!(
                        "engine started — Armed, listening for wake (STT/TTS on-demand)"
                    );
                }
                Ok(EngineCommand::Stop) => continue,
                Ok(EngineCommand::Shutdown) | Err(_) => {
                    end_session(&rt.store, &mut active_session, &mut transcript_len);
                    release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "shutdown");
                    rt.picture.engine = EngineState::Off;
                    rt.picture.set_phase(Phase::Off);
                    return Ok(());
                }
                Ok(EngineCommand::SwitchInput { device_id }) => {
                    apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
                }
                Ok(EngineCommand::SwitchOutput { device_id }) => {
                    apply_output_switch(
                        &mut rt.audio,
                        &mut rt.output_events,
                        &mut rt.picture,
                        &device_id,
                    );
                }
            }
            continue;
        }

        // ── Entry: wake OR freeform follow-up (no second wake) ─────────────
        // Keep last `heard` + `said` while idle so Conversation shows the full
        // last turn (not just Boris). Clear both only when a new utterance starts.
        let capture_kind = if await_reply {
            rt.picture.detail = None;
            rt.picture.turn = None;
            rt.picture.set_phase(Phase::AwaitingReply);
            tracing::info!(
                depth = follow_up_depth,
                "awaiting freeform user reply (no wake)"
            );
            if let Err(e) = hear::settle_after_playback(&rt.mic, &cmd_rx, &mut running) {
                match e {
                    HearBreak::SwitchInput { device_id } => {
                        apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
                        continue;
                    }
                    HearBreak::SwitchOutput { device_id } => {
                        apply_output_switch(
                            &mut rt.audio,
                            &mut rt.output_events,
                            &mut rt.picture,
                            &device_id,
                        );
                        continue;
                    }
                    HearBreak::Stopped if !running => {
                        go_off(
                            &mut rt.picture,
                            &rt.audio,
                            &rt.store,
                            &mut active_session,
                            &mut transcript_len,
                            rt.stt.as_mut(),
                            rt.tts.as_mut(),
                        );
                        continue;
                    }
                    HearBreak::Stopped => {
                        await_reply = false;
                        follow_up_depth = 0;
                        continue;
                    }
                    HearBreak::Disconnected => {
                        end_session(&rt.store, &mut active_session, &mut transcript_len);
                        release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "disconnected");
                        rt.picture.set_phase(Phase::Off);
                        return Ok(());
                    }
                }
            }
            if !running {
                go_off(
                    &mut rt.picture,
                    &rt.audio,
                    &rt.store,
                    &mut active_session,
                    &mut transcript_len,
                    rt.stt.as_mut(),
                    rt.tts.as_mut(),
                );
                continue;
            }
            // Consumed for this entry; may re-arm after this turn if Boris asks again.
            await_reply = false;
            // New freeform utterance — drop previous lines now that we're recording.
            rt.picture.said = None;
            rt.picture.heard = None;
            CaptureKind::AwaitReply
        } else {
            follow_up_depth = 0;
            // Soft landing into Ready: keep last turn text so captions / Conversation
            // don't hard-cut when Speaking ends — clear only after the next wake.
            rt.picture.detail = None;
            rt.picture.turn = None;
            rt.picture.set_phase(Phase::Armed);

            match hear::wait_for_wake(&rt.mic, &mut rt.wake, &cmd_rx, &mut running) {
                Ok(()) => {}
                Err(HearBreak::SwitchInput { device_id }) => {
                    apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
                    continue;
                }
                Err(HearBreak::SwitchOutput { device_id }) => {
                    apply_output_switch(
                        &mut rt.audio,
                        &mut rt.output_events,
                        &mut rt.picture,
                        &device_id,
                    );
                    continue;
                }
                Err(HearBreak::Stopped) if !running => {
                    go_off(
                        &mut rt.picture,
                        &rt.audio,
                        &rt.store,
                        &mut active_session,
                        &mut transcript_len,
                        rt.stt.as_mut(),
                        rt.tts.as_mut(),
                    );
                    continue;
                }
                Err(HearBreak::Stopped) => continue,
                Err(HearBreak::Disconnected) => {
                    end_session(&rt.store, &mut active_session, &mut transcript_len);
                    release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "disconnected");
                    rt.picture.set_phase(Phase::Off);
                    return Ok(());
                }
            }

            if !running {
                go_off(
                    &mut rt.picture,
                    &rt.audio,
                    &rt.store,
                    &mut active_session,
                    &mut transcript_len,
                    rt.stt.as_mut(),
                    rt.tts.as_mut(),
                );
                continue;
            }
            // New user turn — drop previous line now that we're listening again.
            rt.picture.said = None;
            rt.picture.heard = None;
            CaptureKind::AfterWake
        };

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId(next_turn);
        next_turn = next_turn.saturating_add(1);
        rt.picture.turn = Some(turn);
        // Hearing only while the mic is actually recording (not during STT).
        // Preload STT in parallel — should be ready by the time capture ends.
        rt.picture.set_phase(Phase::Hearing);
        tracing::info!(%turn, ?capture_kind, "turn begin — hearing (+ STT preload)");

        let stt_job = spawn_stt_load(rt.stt);
        let capture =
            hear::capture_utterance(&rt.mic, &mut rt.vad, &cmd_rx, &mut running, capture_kind);
        let (stt_owned, stt_load) = join_stt_load(stt_job);
        rt.stt = stt_owned;

        let clip = match capture {
            Ok(c) => c,
            Err(HearBreak::SwitchInput { device_id }) => {
                apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
                continue;
            }
            Err(HearBreak::SwitchOutput { device_id }) => {
                apply_output_switch(
                    &mut rt.audio,
                    &mut rt.output_events,
                    &mut rt.picture,
                    &device_id,
                );
                continue;
            }
            Err(HearBreak::Stopped) if !running => {
                go_off(
                    &mut rt.picture,
                    &rt.audio,
                    &rt.store,
                    &mut active_session,
                    &mut transcript_len,
                    rt.stt.as_mut(),
                    rt.tts.as_mut(),
                );
                continue;
            }
            Err(HearBreak::Stopped) => {
                follow_up_depth = 0;
                continue;
            }
            Err(HearBreak::Disconnected) => {
                end_session(&rt.store, &mut active_session, &mut transcript_len);
                release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "disconnected");
                return Ok(());
            }
        };

        if !running {
            go_off(
                &mut rt.picture,
                &rt.audio,
                &rt.store,
                &mut active_session,
                &mut transcript_len,
                rt.stt.as_mut(),
                rt.tts.as_mut(),
            );
            continue;
        }

        if let Err(e) = stt_load {
            tracing::error!(error = %e, %turn, "stt load failed");
            crate::diagnostics::log_model_load_failure("parakeet", &rt.stt_model_dir, &e);
            let _ = rt.stt.unload();
            rt.picture.detail = Some(format!("stt load: {e}"));
            follow_up_depth = 0;
            rt.picture.set_phase(Phase::Armed);
            continue;
        }

        // Leave Hearing as soon as the mic stops — STT is "Reading", not listening.
        rt.picture.set_phase(Phase::Reading);
        let stt_t = std::time::Instant::now();
        let text = match rt.stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, %turn, "stt failed");
                let _ = rt.stt.unload();
                rt.picture.detail = Some(format!("stt: {e}"));
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
        };
        // Free ~600MB before agent; TTS will preload while the agent runs.
        unload_stt(rt.stt.as_mut(), turn);
        let stt_ms = stt_t.elapsed().as_millis() as u64;
        tracing::info!(
            %turn,
            stt_ms,
            clip_samples = clip.len(),
            clip_ms = (clip.len() as u64 * 1000) / AUDIO_TARGET_RATE as u64,
            "stt done"
        );
        rt.picture.heard = Some(text.clone());
        rt.picture.publish();
        tracing::info!(%turn, %text, "heard");

        // Host guard: skip agent on empty / whitespace / junk transcripts.
        if !transcript_usable(&text) {
            tracing::warn!(
                %turn,
                %text,
                alnum = text.chars().filter(|c| c.is_alphanumeric()).count(),
                "skipping empty/junk transcript — not calling agent"
            );
            rt.picture.detail = Some("didn't catch that".into());
            // If we were in a follow-up, one soft retry is enough; then re-arm.
            if matches!(capture_kind, CaptureKind::AwaitReply) && follow_up_depth < MAX_FOLLOW_UPS
            {
                await_reply = true;
            } else {
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
            }
            continue;
        }

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut rt.audio,
            &mut rt.output_events,
            &mut rt.picture,
        )
        .still_running()
        {
            go_off(
                &mut rt.picture,
                &rt.audio,
                &rt.store,
                &mut active_session,
                &mut transcript_len,
                rt.stt.as_mut(),
                rt.tts.as_mut(),
            );
            continue;
        }

        // Think + preload TTS in parallel (one step ahead of synthesize).
        rt.picture.activity = Some("thinking…".into());
        rt.picture.set_phase(Phase::Thinking);
        // Seed context meter from current conversation size.
        let approx_in = rt
            .agent
            .export_messages()
            .iter()
            .fold(0usize, |acc, m| acc + m.content.to_string().len());
        rt.picture.update_context_from_chars(approx_in + text.len());

        let agent_t = std::time::Instant::now();
        let tts_job = spawn_tts_load(rt.tts);
        rt.agent.set_turn_id(Some(turn.to_string()));
        if let Some(ref sid) = active_session {
            rt.agent.set_session_id(Some(sid.to_string()));
        }

        // Live tool activity → overlay (subscribe emits on this thread inside block_on).
        let activity_bridge =
            std::sync::Arc::new(std::sync::Mutex::new(rt.picture.status_tx.clone()));
        let activity_snap = std::sync::Arc::new(std::sync::Mutex::new((
            rt.picture.engine,
            rt.picture.phase,
            rt.picture.detail.clone(),
            rt.picture.heard.clone(),
            rt.picture.said.clone(),
            rt.picture.mic.clone(),
            rt.picture.speaker.clone(),
            rt.picture.turn,
            rt.picture.context_used,
            rt.picture.context_limit,
        )));
        let snap_w = activity_snap.clone();
        let tx_w = activity_bridge.clone();
        let _unsub = rt.agent.subscribe(move |ev| {
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
                AgentEvent::ToolProgress {
                    tool_name,
                    message,
                    ..
                } => {
                    let msg = message.trim();
                    if msg.is_empty() {
                        Some(format!("tool · {tool_name}"))
                    } else {
                        Some(format!("tool · {tool_name} · {msg}"))
                    }
                }
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

        let outcome = rt.agent_rt.block_on(rt.agent.prompt_with_report(&text));
        _unsub(); // drop live tool listener
        let (tts_owned, tts_load) = join_tts_load(tts_job);
        rt.tts = tts_owned;

        let (outcome, report) = match outcome {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(error = %e, %turn, "agent failed");
                rt.agent.cancel_pending();
                rt.picture.detail = None;
                rt.picture.clear_activity();
                // Recoverable spoken line instead of silent Ready.
                let recovery =
                    "I glitched mid-task. Wake me and say continue if you want me to finish.";
                rt.picture.said = Some(recovery.into());
                rt.picture.publish();
                if rt.tts.load().is_ok() {
                    if let Ok(pcm) = rt.tts.synthesize(recovery) {
                        while rt.output_events.try_recv().is_ok() {}
                        rt.audio.play(pcm);
                        let _ = wait_playback_started(
                            &mut rt.output_events,
                            &cmd_rx,
                            &mut running,
                            &mut rt.audio,
                            &mut rt.picture,
                        );
                        wait_playback_or_stop(
                            &mut rt.output_events,
                            &cmd_rx,
                            &mut running,
                            &mut rt.audio,
                            &mut rt.picture,
                        );
                    }
                    let _ = rt.tts.unload();
                }
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
        };
        rt.picture.activity = if report.tools_used.is_empty() {
            None
        } else {
            Some(format!("{} tools", report.tools_used.len()))
        };
        rt.picture
            .update_context_from_chars(report.approx_chars_in);

        // Resolve HITL confirmations (voice yes/no) before final speech.
        let outcome = match resolve_agent_outcome(
            outcome,
            &mut rt.agent,
            &rt.agent_rt,
            &mut rt.tts,
            &mut rt.stt,
            &rt.mic,
            &mut rt.vad,
            &mut rt.audio,
            &mut rt.output_events,
            &cmd_rx,
            &mut running,
            &mut rt.picture,
            &rt.store,
            &mut active_session,
            &mut transcript_len,
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
            AgentOutcome::Silent | AgentOutcome::Speak { .. } => {
                // Always speak a recovery line — never end the turn in dead silence.
                tracing::warn!(%turn, "agent produced no speech; speaking recovery line");
                (
                    "I didn't get a reply out. Wake me and try again.".to_string(),
                    false,
                )
            }
            AgentOutcome::NeedsConfirmation { .. } => {
                // Should have been resolved above; soft-recover rather than mute.
                tracing::warn!(%turn, "unresolved confirmation after resolve pass");
                (
                    "I needed a yes or no and lost the thread. Wake me and try again.".to_string(),
                    false,
                )
            }
        };

        if let Some(ref sid) = active_session {
            // Full Grok-like transcript: system / user / assistant+tool_calls / tool_result.
            let pairs = agent_message_pairs(&rt.agent);
            match rt.store.sync_messages(sid, &pairs, transcript_len) {
                Ok(n) => transcript_len = n,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %sid,
                        %turn,
                        "session sync_messages failed"
                    );
                }
            }
        }

        // Show reply text while still "Thinking" — TTS synth is NOT speaking yet.
        rt.picture.said = Some(reply.clone());
        rt.picture.publish();
        tracing::info!(%turn, %reply, expect_reply, "said (synth next)");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut rt.audio,
            &mut rt.output_events,
            &mut rt.picture,
        )
        .still_running()
        {
            go_off(
                &mut rt.picture,
                &rt.audio,
                &rt.store,
                &mut active_session,
                &mut transcript_len,
                rt.stt.as_mut(),
                rt.tts.as_mut(),
            );
            continue;
        }

        if let Err(e) = tts_load {
            tracing::error!(error = %e, %turn, "tts load failed");
            crate::diagnostics::log_model_load_failure("supertone", &rt.tts_model_dir, &e);
            let _ = rt.tts.unload();
            rt.picture.detail = Some(format!("tts load: {e}"));
            follow_up_depth = 0;
            rt.picture.set_phase(Phase::Armed);
            continue;
        }

        // Synthesize under Thinking so UI doesn't say "Speaking" with silence.
        let tts_t = std::time::Instant::now();
        let pcm = match rt.tts.synthesize(&reply) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, %turn, "tts failed");
                let _ = rt.tts.unload();
                rt.picture.detail = Some(format!("tts: {e}"));
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
        };
        // Free TTS during playback; optionally preload STT for freeform follow-up.
        unload_tts(rt.tts.as_mut(), turn);
        let tts_ms = tts_t.elapsed().as_millis() as u64;
        let play_samples = pcm.len();
        tracing::info!(%turn, tts_ms, play_samples, "tts synth done");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut rt.audio,
            &mut rt.output_events,
            &mut rt.picture,
        )
        .still_running()
        {
            go_off(
                &mut rt.picture,
                &rt.audio,
                &rt.store,
                &mut active_session,
                &mut transcript_len,
                rt.stt.as_mut(),
                rt.tts.as_mut(),
            );
            continue;
        }

        // Decide follow-up before playback so we can preload STT while speaking.
        let will_await = expect_reply && follow_up_depth < MAX_FOLLOW_UPS;
        // Move STT into a load job (or keep it in the Option) so the compiler
        // always sees a single reclaim path after playback.
        let mut stt_slot: Option<SttBox> = Some(rt.stt);
        let mut stt_follow_job = if will_await {
            Some(spawn_stt_load(stt_slot.take().expect("stt")))
        } else {
            None
        };

        // Queue audio, then flip UI to Talking only when playback has started.
        // Speaker switch mid-play rebuilds the output pipeline and aborts the job —
        // we must not hang waiting for Drained on a dead channel (stuck Talking).
        let play_t = std::time::Instant::now();
        while rt.output_events.try_recv().is_ok() {}
        rt.audio.play(pcm);
        match wait_playback_started(
            &mut rt.output_events,
            &cmd_rx,
            &mut running,
            &mut rt.audio,
            &mut rt.picture,
        ) {
            PlaybackWait::Stopped => {
                rt.stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);
                go_off(
                    &mut rt.picture,
                    &rt.audio,
                    &rt.store,
                    &mut active_session,
                    &mut transcript_len,
                    rt.stt.as_mut(),
                    rt.tts.as_mut(),
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
                rt.picture.set_phase(Phase::Talking);
                tracing::info!(
                    %turn,
                    queue_ms = play_t.elapsed().as_millis() as u64,
                    "playback started — UI Talking"
                );
                wait_playback_or_stop(
                    &mut rt.output_events,
                    &cmd_rx,
                    &mut running,
                    &mut rt.audio,
                    &mut rt.picture,
                );
            }
        }
        let play_ms = play_t.elapsed().as_millis() as u64;

        rt.stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);

        if !running {
            go_off(
                &mut rt.picture,
                &rt.audio,
                &rt.store,
                &mut active_session,
                &mut transcript_len,
                rt.stt.as_mut(),
                rt.tts.as_mut(),
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
            unload_stt(rt.stt.as_mut(), turn);
            unload_tts(rt.tts.as_mut(), turn);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{EngineState, Phase};

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
}
