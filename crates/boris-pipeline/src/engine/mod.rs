//! Voice engine for desktop — sequential turns on one thread.
//!
//! # Module map (contributor navigation)
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`setup`] | One-time audio / wake / agent init |
//! | [`models`] | STT/TTS preload helpers (load-ahead threads) |
//! | [`activity`] | Pure string formatting for the UI activity chip |
//! | [`artifact`] | Session card → `StatusPicture` peek |
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
//!
//! # Shutdown contract
//!
//! 1. Prefer [`Engine::shutdown_and_join`] when the host is exiting (sends
//!    [`EngineCommand::Shutdown`], then joins the engine thread).
//! 2. Dropping [`Engine`] also sends `Shutdown` (best-effort; does **not** join,
//!    so the process should not exit until the thread finishes if you need a
//!    clean model unload — use `shutdown_and_join` for that).
//! 3. [`EngineHandle::shutdown`] alone is enough if another owner holds `Engine`
//!    and will join later.
//! 4. All of the above are safe if the command channel is already closed
//!    (disconnected engine thread): send errors are ignored.

mod activity;
mod artifact;
mod barge;
mod confirm;
mod device_switch;
mod llm;
mod models;
mod outcome;
mod picture;
mod playback;
mod session;
mod setup;
mod speech;
mod turn_trace;
mod util;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::session::types::SessionId;
use boris_agent::{AgentEvent, AgentOutcome};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::TurnId;

use crate::config::PipelineConfig;
use crate::error::{PipelineError, Result};
use crate::hear::{self, CaptureKind, HearBreak};
use crate::status::{EngineState, Phase, StatusPicture};

use activity::activity_label;
use artifact::peek_current;
use barge::{decide_barge_listen, BargeDecision, BargeWatch};
use device_switch::{apply_input_switch, apply_output_switch};
use models::{
    join_stt_load, join_tts_load, lost_tts, maybe_unload_idle, maybe_unload_stt, maybe_unload_tts,
    release_voice_models,
};
use outcome::{resolve_agent_outcome, ConfirmCtx, OutcomeResolve};
use playback::{poll_running, wait_playback_or_stop, wait_playback_started, PlaybackWait};
use session::{begin_session, end_session, enqueue_transcript_sync, go_off};
use setup::{init_runtime, EngineRuntime};
use speech::stream_reply;
use turn_trace::TurnTraceGuard;
use util::{speakable_reply_units, transcript_usable};

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
    SwitchInput {
        device_id: String,
    },
    SwitchOutput {
        device_id: String,
    },
    /// Next `takes` wake hits train the live-mic profile (not a turn).
    StartWakeEnroll {
        takes: u32,
    },
    /// Forget the stored live-mic profile.
    ClearWakeProfile,
}

#[derive(Clone)]
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
}

impl EngineHandle {
    pub fn send(
        &self,
        cmd: EngineCommand,
    ) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.cmd_tx.send(cmd)
    }

    pub fn start(&self) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Start)
    }

    pub fn stop(&self) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Stop)
    }

    pub fn shutdown(&self) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Shutdown)
    }

    pub fn start_wake_enroll(
        &self,
        takes: u32,
    ) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::StartWakeEnroll { takes })
    }

    pub fn clear_wake_profile(&self) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::ClearWakeProfile)
    }
}

/// Join handle + shutdown sender for the engine thread.
///
/// See module-level **Shutdown contract**.
pub struct Engine {
    join: Option<JoinHandle<()>>,
    /// Clone of the command sender used only for Drop / `shutdown_and_join`.
    shutdown_tx: Option<Sender<EngineCommand>>,
}

impl Engine {
    /// Spawn the engine thread. Status snapshots are sent on the returned receiver.
    ///
    /// Returns [`PipelineError::Init`] if the OS refuses the thread spawn.
    /// (Audio / wake init failures surface as a Fault status then thread exit.)
    pub fn spawn(config: PipelineConfig) -> Result<(Self, EngineHandle, Receiver<StatusPicture>)> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let shutdown_tx = cmd_tx.clone();

        let join = thread::Builder::new()
            .name("boris-engine".into())
            .spawn(move || {
                if let Err(e) = run(config, cmd_rx, status_tx) {
                    tracing::error!(error = %e, "engine thread exited with error");
                }
            })
            .map_err(|e| PipelineError::init(format!("spawn boris-engine: {e}")))?;

        Ok((
            Self {
                join: Some(join),
                shutdown_tx: Some(shutdown_tx),
            },
            EngineHandle { cmd_tx },
            status_rx,
        ))
    }

    /// Send [`EngineCommand::Shutdown`] (if the channel is still open) and join
    /// the engine thread. Safe to call if the thread already exited.
    pub fn shutdown_and_join(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(EngineCommand::Shutdown);
        }
        if let Some(join) = self.join.take() {
            if let Err(e) = join.join() {
                tracing::warn!(?e, "engine thread panicked during join");
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Best-effort stop request; do not join here (avoid blocking Drop).
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(EngineCommand::Shutdown);
        }
        // Detach join handle — use `shutdown_and_join` for a synchronous stop.
        if let Some(join) = self.join.take() {
            drop(join);
        }
    }
}

// ── HearBreak / go_off helpers (dedupe turn-loop arms) ───────────────────────

/// How the outer turn loop should react after a hear break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopReact {
    /// `continue` the outer loop (device switch applied or soft stop).
    Continue,
    /// Exit the engine thread cleanly.
    Exit,
}

/// Session fields that live in `run` but are needed by break helpers.
struct SessionRefs<'a> {
    active_session: &'a mut Option<SessionId>,
    transcript_len: &'a mut usize,
}

fn apply_device_cmd(rt: &mut EngineRuntime, cmd: EngineCommand) {
    match cmd {
        EngineCommand::SwitchInput { device_id } => {
            apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
        }
        EngineCommand::SwitchOutput { device_id } => {
            apply_output_switch(
                &mut rt.audio,
                &mut rt.output_events,
                &mut rt.picture,
                &device_id,
            );
        }
        _ => {}
    }
}

fn go_off_session(rt: &mut EngineRuntime, sess: &mut SessionRefs<'_>) {
    go_off(
        &mut rt.picture,
        &rt.audio,
        &rt.store,
        sess.active_session,
        sess.transcript_len,
        rt.stt.as_mut(),
        rt.tts.as_mut(),
        &mut rt.agent,
    );
}

/// Shared HearBreak handling for wake / settle / capture.
///
/// `on_soft_stop` runs when we get `Stopped` while still marked running
/// (clear follow-up depth, etc.).
fn on_hear_break(
    rt: &mut EngineRuntime,
    sess: &mut SessionRefs<'_>,
    br: HearBreak,
    running: bool,
    on_soft_stop: impl FnOnce(),
) -> LoopReact {
    match br {
        HearBreak::SwitchInput { device_id } => {
            apply_input_switch(&mut rt.audio, &mut rt.picture, &device_id);
            LoopReact::Continue
        }
        HearBreak::SwitchOutput { device_id } => {
            apply_output_switch(
                &mut rt.audio,
                &mut rt.output_events,
                &mut rt.picture,
                &device_id,
            );
            LoopReact::Continue
        }
        HearBreak::Stopped if !running => {
            go_off_session(rt, sess);
            LoopReact::Continue
        }
        HearBreak::Stopped => {
            on_soft_stop();
            LoopReact::Continue
        }
        HearBreak::StartWakeEnroll { .. } | HearBreak::ClearWakeProfile => LoopReact::Continue,
        HearBreak::Disconnected => {
            end_session(
                &rt.store,
                sess.active_session,
                sess.transcript_len,
                &mut rt.agent,
            );
            release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "disconnected");
            rt.picture.set_phase(Phase::Off);
            LoopReact::Exit
        }
    }
}

/// Pause leftover speech is already applied. Listen; resume / stop / new turn.
fn listen_after_barge(
    rt: &mut EngineRuntime,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    expect_reply: bool,
) -> std::result::Result<BargeDecision, HearBreak> {
    rt.picture.set_phase(Phase::Hearing);
    rt.picture.activity = Some("barge-in · listening".into());
    rt.picture.publish();
    hear::settle_after_barge(&rt.mic, cmd_rx, running)?;
    let clip = hear::capture_utterance(
        &rt.mic,
        &mut rt.vad,
        cmd_rx,
        running,
        CaptureKind::AfterWake,
    )?;
    let crop = hear::crop_speech(&mut rt.vad, &clip);
    if crop.speech_hops < 10 {
        tracing::info!(
            hops = crop.speech_hops,
            "barge-in listen was silence — resume"
        );
        rt.picture.clear_activity();
        return Ok(BargeDecision::Resume);
    }

    if let Err(e) = rt.stt.load() {
        tracing::warn!(error = %e, "barge-in stt load failed — resuming leftover");
        rt.picture.clear_activity();
        return Ok(BargeDecision::Resume);
    }

    rt.picture.set_phase(Phase::Reading);
    match rt.stt.transcribe(&clip) {
        Ok(text) => {
            tracing::info!(%text, hops = crop.speech_hops, "barge-in heard");
            rt.picture.clear_activity();
            Ok(decide_barge_listen(crop.speech_hops, &text, expect_reply))
        }
        Err(e) => {
            tracing::warn!(error = %e, "barge-in stt failed — resuming leftover");
            rt.picture.clear_activity();
            Ok(BargeDecision::Resume)
        }
    }
}

fn go_off_if_not_running(
    rt: &mut EngineRuntime,
    sess: &mut SessionRefs<'_>,
    running: bool,
) -> bool {
    if !running {
        go_off_session(rt, sess);
        true
    } else {
        false
    }
}

fn run(
    config: PipelineConfig,
    cmd_rx: Receiver<EngineCommand>,
    status_tx: Sender<StatusPicture>,
) -> Result<()> {
    let mut rt = init_runtime(config, status_tx)?;

    let mut running = false;
    let mut next_turn: u64 = 1;
    // When true, next iteration skips wake and freeform-listens for a reply.
    let mut await_reply = false;
    let mut follow_up_depth: u32 = 0;
    let mut active_session: Option<SessionId> = None;
    // How many agent messages are already on disk for `active_session`.
    let mut transcript_len: usize = 0;
    let mut enroll_left: u32 = 0;
    // Transcript from a confirmed barge-in; next loop starts a turn without wake.
    let mut pending_barge_text: Option<String> = None;

    loop {
        let mut sess = SessionRefs {
            active_session: &mut active_session,
            transcript_len: &mut transcript_len,
        };

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
                    rt.picture.activity = None;
                    rt.picture.artifact = None;

                    // STT/TTS stay unloaded until a turn needs them (preloaded one
                    // step ahead during capture / agent — never kept for Armed idle).
                    let start_t = std::time::Instant::now();
                    begin_session(
                        &rt.store,
                        sess.active_session,
                        sess.transcript_len,
                        &mut rt.agent,
                        &rt.system_prompt,
                    );
                    rt.picture.engine = EngineState::On;
                    rt.picture.set_phase(Phase::Armed);
                    tracing::info!(
                        ms = start_t.elapsed().as_millis() as u64,
                        "engine started — Armed, listening for wake (STT/TTS on-demand)"
                    );
                }
                Ok(EngineCommand::Stop) => continue,
                Ok(EngineCommand::Shutdown) | Err(_) => {
                    end_session(
                        &rt.store,
                        sess.active_session,
                        sess.transcript_len,
                        &mut rt.agent,
                    );
                    release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "shutdown");
                    rt.picture.engine = EngineState::Off;
                    rt.picture.set_phase(Phase::Off);
                    return Ok(());
                }
                Ok(
                    cmd @ (EngineCommand::SwitchInput { .. } | EngineCommand::SwitchOutput { .. }),
                ) => {
                    apply_device_cmd(&mut rt, cmd);
                }
                Ok(EngineCommand::StartWakeEnroll { takes }) => {
                    enroll_left = takes.clamp(2, 8);
                    tracing::info!(
                        takes = enroll_left,
                        "wake enroll queued (start engine to record)"
                    );
                    rt.picture
                        .set_wake_enroll(Some(crate::status::WakeEnrollPeek {
                            have: 0,
                            want: enroll_left,
                            ready: false,
                            hint: None,
                        }));
                }
                Ok(EngineCommand::ClearWakeProfile) => {
                    rt.liveness.clear();
                    rt.picture.set_wake_enroll(None);
                    tracing::info!("wake liveness profile cleared");
                }
            }
            continue;
        }

        // ── Entry: wake OR freeform follow-up (no second wake) ─────────────
        // Keep last `heard` + `said` while idle so Conversation shows the full
        // last turn (not just Boris). Clear both only when a new utterance starts.
        let barge_text = pending_barge_text.take();
        let capture_kind = if barge_text.is_some() {
            follow_up_depth = 0;
            await_reply = false;
            rt.picture.detail = None;
            rt.picture.turn = None;
            rt.picture.clear_activity();
            rt.picture.said = None;
            rt.picture.heard = None;
            CaptureKind::AfterWake
        } else if await_reply {
            rt.picture.detail = None;
            rt.picture.turn = None;
            rt.picture.clear_activity();
            rt.picture.set_phase(Phase::AwaitingReply);
            tracing::info!(
                depth = follow_up_depth,
                "awaiting freeform user reply (no wake)"
            );
            if let Err(e) = hear::settle_after_playback(&rt.mic, &cmd_rx, &mut running) {
                match on_hear_break(&mut rt, &mut sess, e, running, || {
                    await_reply = false;
                    follow_up_depth = 0;
                }) {
                    LoopReact::Continue => continue,
                    LoopReact::Exit => return Ok(()),
                }
            }
            if go_off_if_not_running(&mut rt, &mut sess, running) {
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
            // Drop leftover tool activity so the island can idle cleanly.
            rt.picture.detail = None;
            rt.picture.turn = None;
            rt.picture.clear_activity();
            rt.picture.set_phase(Phase::Armed);
            if enroll_left > 0 {
                let have = rt.liveness.take_count() as u32;
                rt.picture
                    .set_wake_enroll(Some(crate::status::WakeEnrollPeek {
                        have,
                        want: have + enroll_left,
                        ready: false,
                        hint: None,
                    }));
            }

            let wake_window =
                match hear::wait_for_wake(&rt.mic, &mut rt.wake, &cmd_rx, &mut running) {
                    Ok(w) => w,
                    Err(HearBreak::StartWakeEnroll { takes }) => {
                        enroll_left = takes.clamp(2, 8);
                        continue;
                    }
                    Err(HearBreak::ClearWakeProfile) => {
                        rt.liveness.clear();
                        enroll_left = 0;
                        rt.picture.set_wake_enroll(None);
                        continue;
                    }
                    Err(e) => match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                        LoopReact::Continue => continue,
                        LoopReact::Exit => return Ok(()),
                    },
                };

            if go_off_if_not_running(&mut rt, &mut sess, running) {
                continue;
            }

            let crop = hear::crop_speech(&mut rt.vad, &wake_window);
            if enroll_left > 0 {
                let want = rt.liveness.take_count() as u32 + enroll_left;
                match rt.liveness.add_take(&crop.pcm, crop.speech_hops, want) {
                    Ok(p) => {
                        enroll_left = want.saturating_sub(p.have);
                        tracing::info!(
                            have = p.have,
                            want = p.want,
                            ready = p.ready,
                            "wake enroll take"
                        );
                        if p.ready {
                            enroll_left = 0;
                        }
                        rt.picture
                            .set_wake_enroll(Some(crate::status::WakeEnrollPeek {
                                have: p.have,
                                want: p.want,
                                ready: p.ready,
                                hint: None,
                            }));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "wake enroll take rejected");
                        let have = rt.liveness.take_count() as u32;
                        rt.picture
                            .set_wake_enroll(Some(crate::status::WakeEnrollPeek {
                                have,
                                want,
                                ready: false,
                                hint: Some(e),
                            }));
                    }
                }
                if let Err(e) = hear::drain_ms(&rt.mic, &cmd_rx, &mut running, 500) {
                    match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                        LoopReact::Continue => continue,
                        LoopReact::Exit => return Ok(()),
                    }
                }
                continue;
            }

            match rt.liveness.classify(&crop.pcm, crop.speech_hops) {
                crate::liveness::WakeOrigin::Playback { z } => {
                    tracing::info!(
                        z,
                        hops = crop.speech_hops,
                        "wake rejected — speaker playback"
                    );
                    if let Err(e) = hear::drain_ms(&rt.mic, &cmd_rx, &mut running, 400) {
                        match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                            LoopReact::Continue => continue,
                            LoopReact::Exit => return Ok(()),
                        }
                    }
                    continue;
                }
                crate::liveness::WakeOrigin::Mismatch { z } => {
                    tracing::info!(
                        z,
                        hops = crop.speech_hops,
                        "wake rejected — not the taught voice"
                    );
                    if let Err(e) = hear::drain_ms(&rt.mic, &cmd_rx, &mut running, 400) {
                        match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                            LoopReact::Continue => continue,
                            LoopReact::Exit => return Ok(()),
                        }
                    }
                    continue;
                }
                crate::liveness::WakeOrigin::TooShort => {
                    tracing::info!(
                        hops = crop.speech_hops,
                        "wake rejected — no speech in window"
                    );
                    if let Err(e) = hear::drain_ms(&rt.mic, &cmd_rx, &mut running, 250) {
                        match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                            LoopReact::Continue => continue,
                            LoopReact::Exit => return Ok(()),
                        }
                    }
                    continue;
                }
                crate::liveness::WakeOrigin::Live => {
                    tracing::debug!(hops = crop.speech_hops, "wake accepted — live speech");
                }
                crate::liveness::WakeOrigin::Unknown => {}
            }

            // New user turn — drop previous line now that we're listening again.
            rt.picture.said = None;
            rt.picture.heard = None;
            rt.picture.wake_enroll = None;
            CaptureKind::AfterWake
        };

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId::new(next_turn);
        next_turn = next_turn.saturating_add(1);
        let trace_start = if matches!(capture_kind, CaptureKind::AfterWake) {
            "wake_hit"
        } else {
            "speech_start"
        };
        let mut turn_trace = TurnTraceGuard::new(
            turn.to_string(),
            sess.active_session.as_ref().map(ToString::to_string),
            rt.maintenance.handle(),
            crate::paths::turn_traces_path(),
            trace_start,
        );
        rt.picture.turn = Some(turn);
        // Overlay glance is this-turn only. The session catalog / Home desk
        // still keep the last card; a new utterance must not resurrect it.
        rt.picture.artifact = None;

        let (text, stt_ms) = if let Some(text) = barge_text {
            turn_trace.mark("barge_in_turn", None);
            tracing::info!(%turn, %text, "turn begin — barge-in transcript");
            rt.picture.heard = Some(text.clone());
            rt.picture.publish();
            (text, 0u64)
        } else {
            // Hearing only while the mic is actually recording (not during STT).
            // Preload STT in parallel — should be ready by the time capture ends.
            rt.picture.set_phase(Phase::Hearing);
            tracing::info!(%turn, ?capture_kind, "turn begin — hearing (+ STT preload)");

            let stt_job = rt.stt_loader.load(rt.stt);
            let capture =
                hear::capture_utterance(&rt.mic, &mut rt.vad, &cmd_rx, &mut running, capture_kind);
            turn_trace.mark("speech_end", None);
            let (stt_owned, stt_load) = join_stt_load(stt_job);
            rt.stt = stt_owned;

            let clip = match capture {
                Ok(c) => c,
                Err(e) => match on_hear_break(&mut rt, &mut sess, e, running, || {
                    follow_up_depth = 0;
                }) {
                    LoopReact::Continue => continue,
                    LoopReact::Exit => return Ok(()),
                },
            };

            if go_off_if_not_running(&mut rt, &mut sess, running) {
                continue;
            }

            if let Err(e) = stt_load {
                turn_trace.mark(
                    "stt_load_error",
                    Some(serde_json::json!({ "error": e.to_string() })),
                );
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
                    turn_trace.mark(
                        "stt_error",
                        Some(serde_json::json!({ "error": e.to_string() })),
                    );
                    tracing::error!(error = %e, %turn, "stt failed");
                    let _ = rt.stt.unload();
                    rt.picture.detail = Some(format!("stt: {e}"));
                    follow_up_depth = 0;
                    rt.picture.set_phase(Phase::Armed);
                    continue;
                }
            };
            // Low-memory evicts STT now; balanced/low-latency keep it warm.
            maybe_unload_stt(rt.stt.as_mut(), turn, rt.residency);
            let stt_ms = stt_t.elapsed().as_millis() as u64;
            turn_trace.span(
                "stt",
                stt_ms,
                Some(serde_json::json!({ "clip_samples": clip.len() })),
            );
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
            (text, stt_ms)
        };

        // Host guard: skip agent on empty / whitespace / junk transcripts.
        if !transcript_usable(&text) {
            turn_trace.mark("transcript_rejected", None);
            tracing::warn!(
                %turn,
                %text,
                alnum = text.chars().filter(|c| c.is_alphanumeric()).count(),
                "skipping empty/junk transcript — not calling agent"
            );
            rt.picture.detail = Some("didn't catch that".into());
            // If we were in a follow-up, one soft retry is enough; then re-arm.
            if matches!(capture_kind, CaptureKind::AwaitReply) && follow_up_depth < MAX_FOLLOW_UPS {
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
            go_off_session(&mut rt, &mut sess);
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
        let tts_job = rt.tts_loader.load(rt.tts);
        rt.agent.set_turn_id(Some(turn.to_string()));
        if let Some(ref sid) = *sess.active_session {
            rt.agent.set_session_id(Some(sid.to_string()));
        }

        // Live tool activity → overlay. Snapshot freezes non-activity fields so
        // mid-turn events do not clobber heard/said with a stale full rebuild.
        let activity_base = std::sync::Arc::new(std::sync::Mutex::new(StatusPicture {
            engine: rt.picture.engine,
            phase: rt.picture.phase,
            detail: rt.picture.detail.clone(),
            heard: rt.picture.heard.clone(),
            said: rt.picture.said.clone(),
            mic: rt.picture.mic.clone(),
            speaker: rt.picture.speaker.clone(),
            turn: rt.picture.turn.map(|t| t.to_string()),
            activity: None,
            thinking: None,
            context_used: rt.picture.context_used,
            context_limit: rt.picture.context_limit,
            artifact: rt.picture.artifact.clone(),
            wake_enroll: None,
        }));
        let activity_tx = rt.picture.status_tx.clone();
        let base_w = activity_base.clone();
        // Recent tool names (for "thinking · after web_search" style labels).
        let recent_tools = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recent_w = recent_tools.clone();
        let art_store = rt.store.clone();
        let art_sid = (*sess.active_session).clone();
        // Keep the listener for the whole turn — including HITL resume —
        // so post-confirm tools / subagents still update the UI.
        let unsub = rt.agent.subscribe(move |ev| {
            // Track tools for post-tool thinking labels.
            if let AgentEvent::ToolExecutionStart { tool_name, .. } = ev {
                if let Ok(mut g) = recent_w.lock() {
                    if !g.iter().any(|t| t == tool_name) {
                        g.push(tool_name.clone());
                    }
                    while g.len() > 4 {
                        g.remove(0);
                    }
                }
            }
            if let AgentEvent::ToolExecutionEnd {
                tool_name,
                ok: true,
                ..
            } = ev
            {
                if tool_name == "present_artifact" {
                    if let Some(sid) = art_sid.as_ref() {
                        if let Some(peek) = peek_current(&art_store, sid) {
                            if let Ok(mut base) = base_w.lock() {
                                base.artifact = Some(peek);
                            }
                        }
                    }
                }
            }
            if let AgentEvent::Reasoning { preview } = ev {
                let Ok(mut base) = base_w.lock() else {
                    return;
                };
                base.thinking = Some(preview.clone());
                let mut snap = base.clone();
                if snap.activity.is_none() {
                    snap.activity = Some("thinking…".into());
                }
                let _ = activity_tx.send(snap);
                return;
            }
            // New LLM round or a tool start — drop stale thoughts so the chip
            // and live preview don't fight.
            if matches!(
                ev,
                AgentEvent::TurnStart { .. }
                    | AgentEvent::ToolExecutionStart { .. }
                    | AgentEvent::NeedsConfirmation { .. }
            ) {
                if let Ok(mut base) = base_w.lock() {
                    base.thinking = None;
                }
            }
            let tools_snapshot = recent_w.lock().ok().map(|g| g.clone()).unwrap_or_default();
            let Some(label) = activity_label(ev, &tools_snapshot) else {
                return;
            };
            let Ok(base) = base_w.lock() else {
                return;
            };
            let mut snap = base.clone();
            snap.activity = Some(label);
            let _ = activity_tx.send(snap);
        });

        let outcome = rt.agent_rt.block_on(rt.agent.prompt_with_report(&text));
        let (tts_owned, tts_load) = join_tts_load(tts_job);
        rt.tts = tts_owned;

        let (outcome, report) = match outcome {
            Ok(pair) => pair,
            Err(e) => {
                turn_trace.span(
                    "agent_error",
                    agent_t.elapsed().as_millis() as u64,
                    Some(serde_json::json!({ "error": e.to_string() })),
                );
                unsub();
                tracing::error!(error = %e, %turn, "agent failed");
                rt.agent.abort();
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
                        if let Err(e) = rt.audio.play(pcm) {
                            tracing::error!(error = %e, "recovery play failed");
                        }
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
        // Live tool labels already mirrored; keep a soft chip until speech/confirm.
        if !report.tools_used.is_empty() {
            rt.picture.activity = Some(format!("{} tools", report.tools_used.len()));
        }
        if report.tools_used.iter().any(|n| n == "present_artifact") {
            if let Some(sid) = sess.active_session.as_ref() {
                rt.picture.artifact = peek_current(&rt.store, sid);
            }
        }
        if !report.tools_used.is_empty() || rt.picture.artifact.is_some() {
            rt.picture.publish();
        }
        rt.picture.update_context_from_chars(report.approx_chars_in);

        // Resolve HITL confirmations (voice yes/no) before final speech.
        let mut confirm = ConfirmCtx {
            agent: &mut rt.agent,
            agent_rt: &rt.agent_rt,
            tts: &mut rt.tts,
            stt: &mut rt.stt,
            mic: &rt.mic,
            vad: &mut rt.vad,
            audio: &mut rt.audio,
            output_events: &mut rt.output_events,
            cmd_rx: &cmd_rx,
            running: &mut running,
            picture: &mut rt.picture,
            store: &rt.store,
            active_session: sess.active_session,
            transcript_len: sess.transcript_len,
            turn,
        };
        let outcome = match resolve_agent_outcome(outcome, &mut confirm) {
            OutcomeResolve::Stopped => {
                unsub();
                continue;
            }
            OutcomeResolve::ReArm => {
                unsub();
                follow_up_depth = 0;
                continue;
            }
            OutcomeResolve::Done(o) => o,
        };
        unsub(); // full turn finished (or speech path next)

        let agent_ms = agent_t.elapsed().as_millis() as u64;
        turn_trace.span(
            "agent",
            agent_ms,
            Some(serde_json::json!({
                "tool_rounds": report.tool_rounds,
                "tools": report.tools_used,
            })),
        );
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

        if let Some(ref sid) = *sess.active_session {
            // Full Grok-like transcript: system / user / assistant+tool_calls / tool_result.
            enqueue_transcript_sync(&rt.store, sid, sess.transcript_len, &rt.agent);
            tracing::debug!(
                session_id = %sid,
                %turn,
                messages = *sess.transcript_len,
                "session sync queued"
            );
        }

        // Show reply text while still "Thinking" — TTS synth is NOT speaking yet.
        // Drop sticky tool chips ("N tools" / last tool label) so Speaking is clean.
        rt.picture.clear_activity();
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
            go_off_session(&mut rt, &mut sess);
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

        // Split at sentence boundaries; synth/play the first unit while later
        // units synthesize. Never speak until we have a final spoken answer
        // (tool-call turns already finished above).
        let mut units = speakable_reply_units(&reply);
        let gap_samples = rt.tts.inter_unit_silence_samples();

        // Decide follow-up before playback. Balanced and low-latency retain
        // both models across an active follow-up chain; low-memory reloads STT
        // while the next utterance is captured.
        let will_await = expect_reply && follow_up_depth < MAX_FOLLOW_UPS;
        let speech_trace_start_ms = turn_trace.elapsed_ms();
        let mut already_audible = false;
        let mut barge_terminal = false;
        let mut speech;
        loop {
            let tts = std::mem::replace(&mut rt.tts, lost_tts());
            let barge_on = rt.barge_in;
            let mut watch = if barge_on {
                Some(BargeWatch::new(&rt.mic, &mut rt.wake))
            } else {
                None
            };
            speech = stream_reply(
                tts,
                units,
                gap_samples,
                turn,
                already_audible,
                &mut rt.audio,
                &mut rt.output_events,
                &cmd_rx,
                &mut running,
                &mut rt.picture,
                &rt.mic,
                watch.as_mut(),
            );
            rt.tts = speech.tts;
            if speech.wait != PlaybackWait::BargedIn {
                break;
            }
            turn_trace.mark("barge_in_pause", None);
            tracing::info!(
                %turn,
                leftover = speech.remaining_units.len(),
                "speech paused for barge-in"
            );
            match listen_after_barge(&mut rt, &cmd_rx, &mut running, will_await) {
                Ok(BargeDecision::Resume) => {
                    tracing::info!(%turn, "barge-in resume — leftover speech");
                    turn_trace.mark("barge_in_resume", None);
                    if let Err(error) = rt.audio.resume() {
                        tracing::warn!(%turn, error = %error, "resume leftover speech failed");
                        rt.audio.stop();
                        speech.wait = PlaybackWait::Aborted;
                        speech.error = Some(error.to_string());
                        break;
                    }
                    units = speech.remaining_units;
                    already_audible = speech.played || already_audible;
                    rt.picture.set_phase(Phase::Talking);
                    continue;
                }
                Ok(BargeDecision::StopTalking) => {
                    tracing::info!(%turn, "barge-in stop — discarding leftover");
                    turn_trace.mark("barge_in_stop", None);
                    rt.audio.stop();
                    maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
                    follow_up_depth = 0;
                    await_reply = false;
                    rt.picture.clear_activity();
                    rt.picture.set_phase(Phase::Armed);
                    barge_terminal = true;
                    break;
                }
                Ok(BargeDecision::TakeTurn(text)) => {
                    tracing::info!(%turn, %text, "barge-in new turn — discarding leftover");
                    turn_trace.mark("barge_in_take_turn", None);
                    rt.audio.stop();
                    maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
                    pending_barge_text = Some(text);
                    follow_up_depth = 0;
                    await_reply = false;
                    rt.picture.clear_activity();
                    speech.wait = PlaybackWait::Aborted;
                    break;
                }
                Err(e) => match on_hear_break(&mut rt, &mut sess, e, running, || {
                    follow_up_depth = 0;
                    await_reply = false;
                }) {
                    LoopReact::Continue => {
                        rt.audio.stop();
                        maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
                        speech.wait = PlaybackWait::Aborted;
                        break;
                    }
                    LoopReact::Exit => return Ok(()),
                },
            }
        }
        maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
        let tts_ms = speech.tts_ms;
        let play_ms = speech.play_ms;
        let speech_ms = speech.speech_ms;
        if speech.queued_samples > 0 {
            let first_audio_ms = speech_trace_start_ms.saturating_add(speech.tts_first_ms);
            turn_trace.mark_at(
                "tts_first_chunk",
                first_audio_ms,
                Some(speech.tts_first_ms),
                Some(serde_json::json!({ "queued_samples": speech.queued_samples })),
            );
        }
        if let Some(started_ms) = speech.audio_started_ms {
            turn_trace.mark_at(
                "audio_started",
                speech_trace_start_ms.saturating_add(started_ms),
                None,
                None,
            );
        }
        turn_trace.span(
            "tts",
            tts_ms,
            Some(serde_json::json!({ "queued_samples": speech.queued_samples })),
        );
        tracing::info!(
            %turn,
            tts_first_ms = speech.tts_first_ms,
            tts_ms,
            play_ms,
            queued_samples = speech.queued_samples,
            "streamed speech complete"
        );
        if pending_barge_text.is_some() || barge_terminal {
            continue;
        }
        match speech.wait {
            PlaybackWait::Stopped => {
                turn_trace.mark("audio_stopped", None);
                go_off_session(&mut rt, &mut sess);
                continue;
            }
            PlaybackWait::Aborted => {
                turn_trace.mark("audio_aborted", None);
                let detail = speech
                    .error
                    .unwrap_or_else(|| "playback interrupted by speaker change".into());
                tracing::warn!(%turn, played = speech.played, %detail, "streamed speech aborted");
                rt.picture.detail = Some(detail);
                follow_up_depth = 0;
                await_reply = false;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
            PlaybackWait::BargedIn => {
                // Handled inside the speak loop; should not reach here.
                follow_up_depth = 0;
                await_reply = false;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
            PlaybackWait::Finished => {
                if let Some(drained_ms) = speech.audio_drained_ms {
                    turn_trace.mark_at(
                        "audio_drained",
                        speech_trace_start_ms.saturating_add(drained_ms),
                        None,
                        None,
                    );
                } else {
                    turn_trace.mark("audio_drained", None);
                }
                if let Some(error) = speech.error {
                    rt.picture.detail = Some(error);
                }
            }
        }

        if go_off_if_not_running(&mut rt, &mut sess, running) {
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
            // No follow-up: low_memory/balanced evict; low_latency keeps models warm.
            maybe_unload_idle(rt.stt.as_mut(), rt.tts.as_mut(), turn, rt.residency);
        }

        tracing::info!(
            %turn,
            stt_ms,
            agent_ms,
            tts_ms,
            play_ms,
            speech_ms,
            // TTS generation and playback overlap; add the speech wall time,
            // not both component durations.
            total_post_capture_ms = stt_ms + agent_ms + speech_ms,
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

    #[test]
    fn loop_react_enum_is_copy() {
        let a = LoopReact::Continue;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(LoopReact::Exit, LoopReact::Exit);
    }
}
