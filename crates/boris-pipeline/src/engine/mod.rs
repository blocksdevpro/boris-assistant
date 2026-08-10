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
use boris_agent::{AgentEvent, AgentOutcome, Role};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::TurnId;

use crate::config::PipelineConfig;
use crate::error::{PipelineError, Result};
use crate::hear::{self, CaptureKind, HearBreak};
use crate::status::{EngineState, Phase, StatusPicture};

use device_switch::{apply_input_switch, apply_output_switch};
use models::{
    join_stt_load, join_tts_load, reclaim_stt_slot, release_voice_models, spawn_stt_load,
    spawn_tts_load, unload_stt, unload_tts, SttBox,
};
use outcome::{resolve_agent_outcome, ConfirmCtx, OutcomeResolve};
use playback::{poll_running, wait_playback_or_stop, wait_playback_started, PlaybackWait};
use session::{agent_message_pairs, begin_session, end_session, go_off};
use setup::{init_runtime, EngineRuntime};
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
    pub fn send(&self, cmd: EngineCommand) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
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

/// Compact tool-activity label for the overlay chip.
///
/// Wire format is stable (`tool ·`, `done ·`, `fail ·`, `thinking ·`, `confirm ·`)
/// so the UI humanizer can parse it. Prefer *what* is happening over step numbers.
///
/// `recent_tools` (most recent last) enriches post-tool thinking labels.
fn activity_label(ev: &AgentEvent, recent_tools: &[String]) -> Option<String> {
    match ev {
        AgentEvent::ToolExecutionStart {
            tool_name,
            args_summary,
            ..
        } => {
            let detail = tool_start_detail(tool_name, args_summary);
            if detail.is_empty() {
                Some(format!("tool · {tool_name}"))
            } else {
                Some(format!("tool · {tool_name} · {detail}"))
            }
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
                let short = truncate_activity(msg, 56);
                Some(format!("tool · {tool_name} · {short}"))
            }
        }
        // Assistant decided on tools — show count before ToolExecutionStart fires.
        AgentEvent::MessageEnd { role, preview }
            if matches!(role, Role::Assistant) && preview.contains("tool call") =>
        {
            let n = preview
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            if n.is_empty() {
                Some("thinking · calling tools".into())
            } else if n == "1" {
                Some("thinking · 1 tool next".into())
            } else {
                Some(format!("thinking · {n} tools next"))
            }
        }
        // Round 0 is the first LLM call (already shown as "thinking…").
        // Later rounds = model deciding after tools — name the last tools when known.
        AgentEvent::TurnStart { round } if *round > 0 => {
            if recent_tools.is_empty() {
                Some("thinking · next action".into())
            } else {
                let names = recent_tools
                    .iter()
                    .rev()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("thinking · after {names}"))
            }
        }
        AgentEvent::NeedsConfirmation { pending } => {
            Some(format!("confirm · {}", pending.name))
        }
        _ => None,
    }
}

/// Prefer a short args hint over the raw `tool (k=v)` audit summary.
fn tool_start_detail(tool_name: &str, args_summary: &str) -> String {
    let s = args_summary.trim();
    if s.is_empty() || s == tool_name {
        return String::new();
    }
    // args_summary is often `bash (command=ls -la)` — strip the name wrapper.
    let inner = s
        .strip_prefix(tool_name)
        .map(str::trim)
        .and_then(|rest| rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')))
        .unwrap_or(s);
    let inner = inner.trim();
    // Prefer the most useful arg for the chip (query / url / goal / command).
    for key in ["query", "url", "goal", "command", "path", "name"] {
        if let Some(v) = extract_arg_value(inner, key) {
            return truncate_activity(&v, 48);
        }
    }
    truncate_activity(inner, 48)
}

/// Pull `key=value` or `key="value"` from a compact args summary.
fn extract_arg_value(summary: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    let idx = summary.find(&needle)?;
    let rest = &summary[idx + needle.len()..];
    if rest.starts_with('"') {
        let body = &rest[1..];
        let end = body.find('"')?;
        let v = body[..end].trim();
        if v.is_empty() {
            return None;
        }
        return Some(v.to_string());
    }
    // Unquoted: until comma or end.
    let end = rest.find(',').unwrap_or(rest.len());
    let v = rest[..end].trim().trim_matches('"');
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn truncate_activity(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
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

                    // STT/TTS stay unloaded until a turn needs them (preloaded one
                    // step ahead during capture / agent — never kept for Armed idle).
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
                Ok(cmd @ (EngineCommand::SwitchInput { .. } | EngineCommand::SwitchOutput { .. })) => {
                    apply_device_cmd(&mut rt, cmd);
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

            match hear::wait_for_wake(&rt.mic, &mut rt.wake, &cmd_rx, &mut running) {
                Ok(()) => {}
                Err(e) => match on_hear_break(&mut rt, &mut sess, e, running, || {}) {
                    LoopReact::Continue => continue,
                    LoopReact::Exit => return Ok(()),
                },
            }

            if go_off_if_not_running(&mut rt, &mut sess, running) {
                continue;
            }
            // New user turn — drop previous line now that we're listening again.
            rt.picture.said = None;
            rt.picture.heard = None;
            CaptureKind::AfterWake
        };

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId::new(next_turn);
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
        let tts_job = spawn_tts_load(rt.tts);
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
            context_used: rt.picture.context_used,
            context_limit: rt.picture.context_limit,
        }));
        let activity_tx = rt.picture.status_tx.clone();
        let base_w = activity_base.clone();
        // Recent tool names (for "thinking · after web_search" style labels).
        let recent_tools = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recent_w = recent_tools.clone();
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
            let tools_snapshot = recent_w
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
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
            rt.picture.publish();
        }
        rt.picture
            .update_context_from_chars(report.approx_chars_in);

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
            let pairs = agent_message_pairs(&rt.agent);
            match rt.store.sync_messages(sid, &pairs, *sess.transcript_len) {
                Ok(n) => *sess.transcript_len = n,
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
            go_off_session(&mut rt, &mut sess);
            continue;
        }

        // Decide follow-up before playback so we can preload STT while speaking.
        let will_await = expect_reply && follow_up_depth < MAX_FOLLOW_UPS;
        // Move STT into a load job (or keep it in the Option) so the compiler
        // always sees a single reclaim path after playback.
        let mut stt_slot: Option<SttBox> = Some(rt.stt);
        let mut stt_follow_job = if will_await {
            Some(spawn_stt_load(
                stt_slot
                    .take()
                    .expect("stt slot present when starting follow-up preload"),
            ))
        } else {
            None
        };

        // Queue audio, then flip UI to Talking only when playback has started.
        // Speaker switch mid-play rebuilds the output pipeline and aborts the job —
        // we must not hang waiting for Drained on a dead channel (stuck Talking).
        let play_t = std::time::Instant::now();
        while rt.output_events.try_recv().is_ok() {}
        if let Err(e) = rt.audio.play(pcm) {
            tracing::error!(error = %e, %turn, "play failed");
            rt.stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);
            follow_up_depth = 0;
            rt.picture.detail = Some(format!("play: {e}"));
            rt.picture.set_phase(Phase::Armed);
            continue;
        }
        match wait_playback_started(
            &mut rt.output_events,
            &cmd_rx,
            &mut running,
            &mut rt.audio,
            &mut rt.picture,
        ) {
            PlaybackWait::Stopped => {
                rt.stt = reclaim_stt_slot(&mut stt_slot, &mut stt_follow_job);
                go_off_session(&mut rt, &mut sess);
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

    #[test]
    fn activity_label_tools() {
        let empty: &[String] = &[];
        let start = AgentEvent::ToolExecutionStart {
            call_id: "1".into(),
            tool_name: "bash".into(),
            args_summary: String::new(),
        };
        assert_eq!(activity_label(&start, empty).as_deref(), Some("tool · bash"));

        let start_args = AgentEvent::ToolExecutionStart {
            call_id: "2".into(),
            tool_name: "bash".into(),
            args_summary: "bash (command=ls -la)".into(),
        };
        assert_eq!(
            activity_label(&start_args, empty).as_deref(),
            Some("tool · bash · ls -la")
        );

        let search = AgentEvent::ToolExecutionStart {
            call_id: "3".into(),
            tool_name: "web_search".into(),
            args_summary: "web_search (query=Uttam LinkedIn Dhanbad)".into(),
        };
        assert_eq!(
            activity_label(&search, empty).as_deref(),
            Some("tool · web_search · Uttam LinkedIn Dhanbad")
        );

        let end_ok = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: true,
            duration_ms: 1,
        };
        assert_eq!(activity_label(&end_ok, empty).as_deref(), Some("done · bash"));

        let end_fail = AgentEvent::ToolExecutionEnd {
            call_id: "1".into(),
            tool_name: "bash".into(),
            ok: false,
            duration_ms: 1,
        };
        assert_eq!(
            activity_label(&end_fail, empty).as_deref(),
            Some("fail · bash")
        );

        let noise = AgentEvent::TurnStart { round: 0 };
        assert!(activity_label(&noise, empty).is_none());

        let round2 = AgentEvent::TurnStart { round: 1 };
        assert_eq!(
            activity_label(&round2, empty).as_deref(),
            Some("thinking · next action")
        );
        let after = vec!["web_search".into(), "web_fetch".into()];
        assert_eq!(
            activity_label(&round2, &after).as_deref(),
            Some("thinking · after web_search, web_fetch")
        );

        let tools_next = AgentEvent::MessageEnd {
            role: Role::Assistant,
            preview: "3 tool call(s)".into(),
        };
        assert_eq!(
            activity_label(&tools_next, empty).as_deref(),
            Some("thinking · 3 tools next")
        );
    }

    #[test]
    fn loop_react_enum_is_copy() {
        let a = LoopReact::Continue;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(LoopReact::Exit, LoopReact::Exit);
    }
}
