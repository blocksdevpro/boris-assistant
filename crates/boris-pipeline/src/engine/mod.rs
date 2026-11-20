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
//! | [`think`] | Agent turn off-thread + thinking barge-in |
//! | [`util`] | Small pure helpers |
//!
//! # Turn loop
//!
//! ```text
//! Start → Armed → (wake | await reply) → hear → read → think → talk → Armed → …
//! ```
//!
//! Wake scoring, VAD capture, STT, and TTS stay on the engine thread. The
//! agent turn runs on a scoped thread during Thinking so the engine can still
//! service Stop and wake barge-in. Status is pushed for the UI. Hosts send
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
mod chime;
mod confirm;
mod device_switch;
mod device_watch;
mod early_speech;
mod llm;
mod models;
mod outcome;
mod picture;
mod playback;
mod self_check;
mod session;
mod setup;
mod speech;
mod think;
mod turn_trace;
mod util;

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::{AgentErrorKind, AgentEvent, AgentOutcome, SessionId};
use boris_audio::AUDIO_TARGET_RATE;
use boris_core::TurnId;

use crate::config::PipelineConfig;
use crate::error::{PipelineError, Result};
use crate::hear::{self, CaptureKind, HearBreak};
use crate::status::{EngineState, Phase, StatusPicture};

use activity::{activity_label, is_tool_chip, note_tool_start};
use artifact::peek_current;
use barge::{BargeDecision, BargeWatch};
use device_switch::{apply_input_switch, apply_output_switch};
use models::{
    join_stt_load, lost_tts, maybe_unload_idle, maybe_unload_stt, maybe_unload_tts,
    release_voice_models,
};
use outcome::{resolve_agent_outcome, ConfirmCtx, OutcomeResolve, ResolveDone};
use playback::{poll_running, PlaybackWait};
use session::{begin_session, end_session, enqueue_transcript_sync, go_off};
use setup::{init_runtime, EngineRuntime};
use speech::stream_reply;
use think::{
    run_thinking, AgentWork, EarlySpeech, ThinkCtx, ThinkResolve, ThinkSpeech, ThinkTiming,
};
use turn_trace::TurnTraceGuard;
use util::{speakable_reply_units, transcript_usable};

/// Mic fan-out queue. Must stay large enough that a brief engine stall during
/// capture (logging, status publish) never drops live speech frames.
pub(super) const MIC_QUEUE: usize = 256;

/// Max freeform follow-ups without re-wake (name, choice, full sentence, yes/no, …).
/// Multi-step voice chores need more than a couple of back-and-forths.
const MAX_FOLLOW_UPS: u32 = 24;

/// A barge-in starts a fresh user turn. Thinking replacements additionally
/// carry the cancelled objective as host-only reference, never as user text.
struct PendingBargeTurn {
    user_text: String,
    interrupted_text: Option<String>,
}

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
    /// Run the on-device self-check while idle (ignored mid-turn).
    RunSelfCheck,
    /// Typed / pasted value for a pending collect_input pause.
    SubmitInput {
        id: String,
        value: String,
    },
    /// User dismissed the on-screen input field.
    CancelInput {
        id: String,
    },
}

#[derive(Clone)]
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
    /// Devices the user picked; the device watcher returns to them after an
    /// unplug and otherwise follows the OS default.
    pins: device_watch::SharedPins,
}

impl EngineHandle {
    /// Send a command. `SwitchInput` / `SwitchOutput` from the host also
    /// record the pick (`"default"` / empty = follow the OS default).
    pub fn send(
        &self,
        cmd: EngineCommand,
    ) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        match &cmd {
            EngineCommand::SwitchInput { device_id } => {
                self.pins().input = device_watch::pin_from_request(device_id);
            }
            EngineCommand::SwitchOutput { device_id } => {
                self.pins().output = device_watch::pin_from_request(device_id);
            }
            _ => {}
        }
        self.cmd_tx.send(cmd)
    }

    fn pins(&self) -> std::sync::MutexGuard<'_, device_watch::DevicePins> {
        // Two `Option<String>`s: a panic mid-write cannot leave them invalid.
        self.pins.lock().unwrap_or_else(|p| p.into_inner())
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

    /// Run the on-device self-check; progress arrives as `StatusPicture.self_check`.
    pub fn run_self_check(&self) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::RunSelfCheck)
    }

    pub fn submit_input(
        &self,
        id: String,
        value: String,
    ) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::SubmitInput { id, value })
    }

    pub fn cancel_input(
        &self,
        id: String,
    ) -> std::result::Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::CancelInput { id })
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
        let pins = device_watch::SharedPins::default();
        let (watch_tx, watch_pins) = (cmd_tx.clone(), pins.clone());
        let crash_tx = status_tx.clone();
        let (mic_label, speaker_label) = (config.mic_label.clone(), config.speaker_label.clone());

        let join = thread::Builder::new()
            .name("boris-engine".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(config, cmd_rx, status_tx, watch_tx, watch_pins)
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::error!(error = %e, "engine thread exited with error"),
                    Err(payload) => {
                        // Without this the UI keeps the last snapshot (often
                        // "Starting · initializing…") forever.
                        let reason =
                            boris_core::panic_message(payload.as_ref()).unwrap_or("internal error");
                        tracing::error!(panic = %reason, "engine thread panicked");
                        // `u64::MAX` outranks any snapshot this engine sent.
                        let _ = crash_tx.send(setup::fault_picture(
                            u64::MAX,
                            format!("Boris stopped unexpectedly: {reason}"),
                            mic_label,
                            speaker_label,
                        ));
                    }
                }
            })
            .map_err(|e| PipelineError::init(format!("spawn boris-engine: {e}")))?;

        Ok((
            Self {
                join: Some(join),
                shutdown_tx: Some(shutdown_tx),
            },
            EngineHandle { cmd_tx, pins },
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

/// How often host-line TTS loading polls host commands.
const HOST_LINE_POLL: std::time::Duration = std::time::Duration::from_millis(20);
/// After a reminder line that was not heard, wait this long before retrying.
const REMINDER_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// What the turn loop does after a host line.
#[derive(Debug)]
enum AfterHostLine {
    /// Carry on.
    Proceed,
    /// Barge-in "stop": re-arm.
    Dismissed,
    /// Barge-in with a new request: start a turn with it.
    TakeTurn(String),
    /// A hear break during the barge-in listen (stop, device switch, …).
    Break(HearBreak),
}

struct HostLine {
    /// The line became audible and did not fail part-way (a barge-in or a
    /// Stop after it started still counts).
    heard: bool,
    next: AfterHostLine,
}

/// Speak one short host-authored line: due reminders, host-intent
/// acknowledgements, recovery lines. The TTS load (loader thread) and the
/// synthesis (stream helper) run off the engine thread while host commands
/// are polled. Playback has the Talking barge watch, and a barge-in is
/// handled like one during a reply. The engine is idle (Armed) afterwards,
/// so TTS residency follows the idle rule of a turn that ends without a
/// follow-up.
fn speak_host_line(
    rt: &mut EngineRuntime,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    line: &str,
    turn: TurnId,
) -> HostLine {
    let mut out = HostLine {
        heard: false,
        next: AfterHostLine::Proceed,
    };
    rt.picture.said = Some(line.into());
    rt.picture.publish();

    let mut slot = models::TtsSlot::Loading(
        rt.tts_loader
            .load(std::mem::replace(&mut rt.tts, lost_tts())),
    );
    let mut stopped = false;
    while slot.is_loading() {
        if !poll_running(
            cmd_rx,
            running,
            &mut rt.audio,
            &mut rt.output_events,
            &mut rt.picture,
        )
        .still_running()
        {
            stopped = true;
            break;
        }
        thread::sleep(HOST_LINE_POLL);
    }
    let loaded = if stopped { None } else { slot.take_loaded() };
    let Some(mut tts) = loaded else {
        // Stop during the load still has to wait for the model to return.
        let (tts, result) = slot.join();
        rt.tts = tts;
        if let (false, Err(e)) = (stopped, result) {
            tracing::error!(error = %e, %line, "host line: tts load failed");
            let _ = rt.tts.unload();
        }
        rt.picture.said = None;
        rt.picture.publish();
        return out;
    };

    let gap_samples = tts.inter_unit_silence_samples();
    let mut units = speakable_reply_units(line);
    let mut already_audible = false;
    loop {
        let mut watch = if rt.barge_in {
            Some(BargeWatch::new(&rt.mic, &mut rt.wake))
        } else {
            None
        };
        let mut speech = stream_reply(
            tts,
            speech::SpeechUnits::Fixed(units),
            gap_samples,
            turn,
            already_audible,
            &mut rt.audio,
            &mut rt.output_events,
            cmd_rx,
            running,
            &mut rt.picture,
            &rt.mic,
            watch.as_mut(),
        );
        tts = std::mem::replace(&mut speech.tts, lost_tts());
        match speech.wait {
            PlaybackWait::Finished => {
                out.heard = speech.error.is_none();
                if let Some(e) = speech.error {
                    tracing::error!(error = %e, %line, "host line cut short");
                }
                break;
            }
            PlaybackWait::Stopped => {
                out.heard = already_audible || speech.played;
                break;
            }
            PlaybackWait::Aborted => {
                tracing::error!(error = ?speech.error, %line, "host line not played");
                break;
            }
            PlaybackWait::BargedIn => {
                out.heard = true;
                match listen_after_barge(rt, cmd_rx, running, false) {
                    Ok(BargeDecision::Resume) => {
                        if rt.audio.resume().is_err() {
                            rt.audio.stop();
                            break;
                        }
                        units = speech.remaining_units;
                        already_audible = true;
                        rt.picture.set_phase(Phase::Talking);
                    }
                    Ok(BargeDecision::StopTalking) => {
                        rt.audio.stop();
                        out.next = AfterHostLine::Dismissed;
                        break;
                    }
                    Ok(BargeDecision::TakeTurn(text)) => {
                        rt.audio.stop();
                        out.next = AfterHostLine::TakeTurn(text);
                        break;
                    }
                    Err(e) => {
                        rt.audio.stop();
                        out.next = AfterHostLine::Break(e);
                        break;
                    }
                }
            }
        }
    }
    rt.tts = tts;
    if rt.residency.should_evict_idle() {
        models::unload_tts(rt.tts.as_mut(), turn);
    }
    if !out.heard {
        rt.picture.said = None;
        rt.picture.publish();
    }
    out
}

/// Apply how a host line ended to the turn loop; the caller then
/// `continue`s (or exits on [`LoopReact::Exit`]).
fn after_host_line(
    rt: &mut EngineRuntime,
    sess: &mut SessionRefs<'_>,
    next: AfterHostLine,
    running: bool,
    pending_barge_turn: &mut Option<PendingBargeTurn>,
    await_reply: &mut bool,
    follow_up_depth: &mut u32,
) -> LoopReact {
    match next {
        AfterHostLine::Break(e) => on_hear_break(rt, sess, e, running, || {
            *await_reply = false;
            *follow_up_depth = 0;
        }),
        _ if !running => {
            go_off_session(rt, sess);
            LoopReact::Continue
        }
        AfterHostLine::Proceed => LoopReact::Continue,
        AfterHostLine::Dismissed => {
            *await_reply = false;
            *follow_up_depth = 0;
            LoopReact::Continue
        }
        AfterHostLine::TakeTurn(user_text) => {
            *pending_barge_turn = Some(PendingBargeTurn {
                user_text,
                interrupted_text: None,
            });
            *await_reply = false;
            *follow_up_depth = 0;
            LoopReact::Continue
        }
    }
}

/// True when the engine loop is in a gap between turns where a due reminder
/// may be spoken: Armed only. Never mid-turn (Hearing / Reading / Thinking /
/// Talking), in the follow-up window (AwaitingReply: the user may already be
/// answering, and the line would talk over them), during an approval prompt
/// (AwaitingConfirm, or any confirmation still pending in the agent), while
/// typed input is awaited (AwaitingInput), or when a barge-in turn is queued.
/// A follow-up window that ends without a reply re-arms, so a reminder due
/// during a turn is spoken right after it.
fn reminder_gap_open(phase: Phase, confirmation_pending: bool, barge_turn_queued: bool) -> bool {
    phase == Phase::Armed && !confirmation_pending && !barge_turn_queued
}

/// The items due at `now_ms` when the gap is open, without removing them:
/// they are taken from the store only once their line was heard.
fn due_at_gap(
    reminders: &boris_agent::ReminderStore,
    gap_open: bool,
    now_ms: i64,
) -> Vec<boris_agent::Reminder> {
    if !gap_open {
        return Vec::new();
    }
    reminders
        .list()
        .into_iter()
        .filter(|item| item.due_unix_ms <= now_ms)
        .collect()
}

/// After the reminder line: a heard line removes exactly the spoken items
/// (`take_due` at the same `now_ms`; new items are always due later) and
/// clears the retry backoff. A line that was not heard keeps them due and
/// backs off, so a broken speaker cannot spin the Armed wait. Returns the
/// removed items.
fn settle_reminders(
    reminders: &boris_agent::ReminderStore,
    heard: bool,
    now_ms: i64,
    retry_at: &mut Option<std::time::Instant>,
) -> Vec<boris_agent::Reminder> {
    if heard {
        *retry_at = None;
        reminders.take_due(now_ms)
    } else {
        *retry_at = Some(std::time::Instant::now() + REMINDER_RETRY_AFTER);
        Vec::new()
    }
}

/// Whether the Armed wait should yield for due reminders now: the same gap
/// rule as [`deliver_reminders`] and no pending retry backoff.
fn reminders_ready(gap_open: bool, retry_at: Option<std::time::Instant>, has_due: bool) -> bool {
    gap_open && retry_at.is_none_or(|at| std::time::Instant::now() >= at) && has_due
}

/// Speak every due timer and reminder (earliest first) as one host line, at
/// a gap between turns only ([`reminder_gap_open`]); a due item otherwise
/// waits for the current turn to end. Playback follows [`speak_host_line`]
/// (Talking barge watch, Stop polled). A heard line is then recorded in the
/// conversation as a host event and synced to the transcript, so a
/// follow-up "what was that reminder?" has the answer; a line that was not
/// heard stays due and is retried after [`REMINDER_RETRY_AFTER`].
fn deliver_reminders(
    rt: &mut EngineRuntime,
    sess: &mut SessionRefs<'_>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
) -> AfterHostLine {
    let now = boris_agent::now_unix_ms();
    let gap = reminder_gap_open(rt.picture.phase, rt.agent.has_pending_confirmation(), false);
    let due = due_at_gap(&rt.reminders, gap, now);
    if due.is_empty() {
        return AfterHostLine::Proceed;
    }
    let line = due
        .iter()
        .map(|item| item.spoken(now))
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!(count = due.len(), %line, "delivering due reminders");
    rt.picture.activity = Some(if due.len() == 1 {
        "reminder".into()
    } else {
        format!("{} reminders", due.len())
    });
    let spoken = speak_host_line(rt, cmd_rx, running, &line, TurnId::new(0));
    let taken = settle_reminders(&rt.reminders, spoken.heard, now, &mut rt.reminder_retry_at);
    if taken.is_empty() {
        tracing::error!(
            %line,
            retry_secs = REMINDER_RETRY_AFTER.as_secs(),
            "reminder line not heard; kept due for a retry"
        );
    } else if rt
        .agent
        .record_host_event(&boris_agent::spoken_reminders_note(&line, now))
    {
        if let Some(ref sid) = *sess.active_session {
            enqueue_transcript_sync(&rt.store, sid, sess.transcript_len, &rt.agent);
        }
    }
    rt.picture.clear_activity();
    rt.picture.publish();
    spoken.next
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

/// Trace when early speech of one Thinking pass became audible. `base_ms` is
/// the pass start on the turn clock.
fn trace_early_audio(turn_trace: &mut TurnTraceGuard, early: &EarlySpeech, base_ms: u64) {
    if let Some(at) = early.progress_audio_ms {
        turn_trace.mark_at(
            "progress_audio_started",
            base_ms.saturating_add(at),
            None,
            None,
        );
    }
    if let Some(at) = early.cancelled_audio_ms {
        turn_trace.mark_at(
            "early_audio_started",
            base_ms.saturating_add(at),
            None,
            None,
        );
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
        HearBreak::StartWakeEnroll { .. }
        | HearBreak::ClearWakeProfile
        | HearBreak::RunSelfCheck
        | HearBreak::RemindersDue => LoopReact::Continue,
        HearBreak::Disconnected => {
            end_session(
                &rt.store,
                sess.active_session,
                sess.transcript_len,
                &mut rt.agent,
            );
            release_voice_models(rt.stt.as_mut(), rt.tts.as_mut(), "disconnected");
            rt.picture.mark_devices_dead();
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
    barge::listen_after_pause(
        &rt.mic,
        &mut rt.vad,
        &mut rt.stt,
        &mut rt.picture,
        cmd_rx,
        running,
        expect_reply,
    )
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
    watch_tx: Sender<EngineCommand>,
    pins: device_watch::SharedPins,
) -> Result<()> {
    let mut rt = init_runtime(config, status_tx)?;
    // Recovers from unplugged / re-plugged devices for the engine's lifetime.
    let _device_watch = device_watch::DeviceWatch::spawn(rt.audio.health(), pins, watch_tx);

    let mut running = false;
    let mut next_turn: u64 = 1;
    // When true, next iteration skips wake and freeform-listens for a reply.
    let mut await_reply = false;
    let mut follow_up_depth: u32 = 0;
    let mut active_session: Option<SessionId> = None;
    // How many agent messages are already on disk for `active_session`.
    let mut transcript_len: usize = 0;
    let mut enroll_left: u32 = 0;
    // Confirmed barge-in; next loop starts a clean turn without another wake.
    let mut pending_barge_turn: Option<PendingBargeTurn> = None;

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
                    rt.picture.clear_artifacts();
                    // A fresh Start re-marks devices alive: a prior
                    // disconnect-dead flag must not linger once the host
                    // restarts the engine on (possibly fixed) devices.
                    rt.picture.mark_devices_alive();

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
                Ok(EngineCommand::RunSelfCheck) => {
                    self_check::run_self_check(&mut rt);
                }
                Ok(EngineCommand::SubmitInput { .. } | EngineCommand::CancelInput { .. }) => {}
            }
            continue;
        }

        // ── Entry: wake OR freeform follow-up (no second wake) ─────────────
        // Keep last `heard` + `said` while idle so Conversation shows the full
        // last turn (not just Boris). Clear both only when a new utterance starts.
        let barge_turn = pending_barge_turn.take();
        let capture_kind = if barge_turn.is_some() {
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
            // Due reminders wait for the next Armed wait: the user may
            // already be answering, and a reminder here would talk over them.
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

            let mic_known_silent =
                rt.picture.listen_hint == Some(crate::status::ListenHint::MicSilent);
            // Same gap rule and retry backoff as `deliver_reminders`, so a
            // due item it would not take can never make this wait spin.
            let reminder_gap =
                reminder_gap_open(Phase::Armed, rt.agent.has_pending_confirmation(), false);
            let reminder_retry_at = rt.reminder_retry_at;
            let picture = &mut rt.picture;
            let reminders = &rt.reminders;
            let wake_window = match hear::wait_for_wake(
                &rt.mic,
                &mut rt.wake,
                &cmd_rx,
                &mut running,
                mic_known_silent,
                |signal| match signal {
                    hear::MicSignal::Silent => {
                        picture.set_listen_hint(Some(crate::status::ListenHint::MicSilent))
                    }
                    hear::MicSignal::Live => picture.set_listen_hint(None),
                },
                || {
                    reminders_ready(
                        reminder_gap,
                        reminder_retry_at,
                        reminders.has_due(boris_agent::now_unix_ms()),
                    )
                },
            ) {
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
                Err(HearBreak::RunSelfCheck) => {
                    self_check::run_self_check(&mut rt);
                    continue;
                }
                Err(HearBreak::RemindersDue) => {
                    let next = deliver_reminders(&mut rt, &mut sess, &cmd_rx, &mut running);
                    match after_host_line(
                        &mut rt,
                        &mut sess,
                        next,
                        running,
                        &mut pending_barge_turn,
                        &mut await_reply,
                        &mut follow_up_depth,
                    ) {
                        LoopReact::Continue => continue,
                        LoopReact::Exit => return Ok(()),
                    }
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
                    rt.picture
                        .set_listen_hint(Some(crate::status::ListenHint::IgnoredPlayback));
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
                    rt.picture
                        .set_listen_hint(Some(crate::status::ListenHint::IgnoredVoice));
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

            // Wake accepted: an audible "listening" cue before capture starts.
            // Fire-and-forget; AEC removes it from the capture, and the reply
            // path drains stale output events before it plays.
            if let Some(chime) = &rt.wake_chime {
                if let Err(e) = rt.audio.play(chime.clone()) {
                    tracing::debug!(error = %e, "wake chime not played");
                }
            }

            // New user turn — drop previous line now that we're listening again.
            rt.picture.said = None;
            rt.picture.heard = None;
            rt.picture.wake_enroll = None;
            rt.picture.listen_hint = None;
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
        rt.picture.clear_artifacts();

        let (text, stt_ms, interrupted_text, cut_by_cap) = if let Some(barge) = barge_turn {
            let text = barge.user_text;
            turn_trace.mark("barge_in_turn", None);
            tracing::info!(%turn, %text, "turn begin — barge-in transcript");
            rt.picture.heard = Some(text.clone());
            rt.picture.publish();
            (text, 0u64, barge.interrupted_text, false)
        } else {
            // Hearing only while the mic is actually recording (not during STT).
            rt.picture.set_phase(Phase::Hearing);
            let partial_cfg = crate::partials::PartialConfig::from_env();
            // Live partials need the model in place *during* capture, so they
            // only run when STT is already warm (balanced/low_latency after
            // the first turn). Cold turns keep the preload-then-decode path
            // below so capture still overlaps the load.
            let streaming = partial_cfg.enabled
                && rt.stt.is_loaded()
                && !matches!(capture_kind, CaptureKind::AwaitConfirm);

            let clip = if streaming {
                tracing::info!(%turn, ?capture_kind, "turn begin — hearing (+ live STT partials)");
                let mut partial_count = 0u32;
                let capture = hear::capture_utterance_with_partials(
                    &rt.mic,
                    &mut rt.vad,
                    &cmd_rx,
                    &mut running,
                    capture_kind,
                    &partial_cfg,
                    rt.stt.as_mut(),
                    |text, snapshot_samples| {
                        partial_count += 1;
                        turn_trace.mark(
                            "stt_partial",
                            Some(serde_json::json!({
                                "n": partial_count,
                                "chars": text.chars().count(),
                                "snapshot_samples": snapshot_samples,
                            })),
                        );
                        // Live overlay line; the final transcribe below stays
                        // authoritative and republishes.
                        rt.picture.heard = Some(text.to_string());
                        rt.picture.publish();
                    },
                );
                turn_trace.mark("speech_end", None);
                match capture {
                    Ok(c) => c,
                    Err(e) => match on_hear_break(&mut rt, &mut sess, e, running, || {
                        follow_up_depth = 0;
                    }) {
                        LoopReact::Continue => continue,
                        LoopReact::Exit => return Ok(()),
                    },
                }
            } else {
                // Preload STT in parallel — should be ready by the time capture ends.
                tracing::info!(%turn, ?capture_kind, "turn begin — hearing (+ STT preload)");

                let stt_job = rt.stt_loader.load(rt.stt);
                let capture = hear::capture_utterance(
                    &rt.mic,
                    &mut rt.vad,
                    &cmd_rx,
                    &mut running,
                    capture_kind,
                );
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
                clip
            };

            if go_off_if_not_running(&mut rt, &mut sess, running) {
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
            let cut_by_cap = crate::hear::clip_hit_cap(clip.len(), capture_kind);
            (text, stt_ms, None, cut_by_cap)
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
            rt.picture.activity = Some("didn't catch that — wake me and try again".into());
            // Hint goes to `activity`, not `detail`: the overlay renders
            // `detail` as an error and a missed utterance is transient.
            // If we were in a follow-up, one soft retry is enough; then re-arm.
            if matches!(capture_kind, CaptureKind::AwaitReply) && follow_up_depth < MAX_FOLLOW_UPS {
                await_reply = true;
            } else {
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
            }
            continue;
        }

        // Host intents: a dismissal or a new-session request needs no model
        // round (a ~5 s LLM call just to say "Stopped." otherwise).
        if interrupted_text.is_none() {
            let fresh = matches!(capture_kind, CaptureKind::AfterWake);
            match barge::host_intent(&text, fresh) {
                Some(barge::HostIntent::Dismiss) => {
                    turn_trace.mark("host_dismiss", None);
                    tracing::info!(%turn, %text, "host intent: dismiss — no agent call");
                    rt.picture.activity = Some("okay — standing by".into());
                    follow_up_depth = 0;
                    await_reply = false;
                    rt.picture.set_phase(Phase::Armed);
                    continue;
                }
                Some(barge::HostIntent::NewSession) => {
                    turn_trace.mark("host_new_session", None);
                    tracing::info!(%turn, %text, "host intent: new session — no agent call");
                    end_session(
                        &rt.store,
                        sess.active_session,
                        sess.transcript_len,
                        &mut rt.agent,
                    );
                    begin_session(
                        &rt.store,
                        sess.active_session,
                        sess.transcript_len,
                        &mut rt.agent,
                        &rt.system_prompt,
                    );
                    rt.picture.clear_artifacts();
                    let spoken =
                        speak_host_line(&mut rt, &cmd_rx, &mut running, "Okay, fresh start.", turn);
                    follow_up_depth = 0;
                    await_reply = false;
                    rt.picture.set_phase(Phase::Armed);
                    match after_host_line(
                        &mut rt,
                        &mut sess,
                        spoken.next,
                        running,
                        &mut pending_barge_turn,
                        &mut await_reply,
                        &mut follow_up_depth,
                    ) {
                        LoopReact::Continue => continue,
                        LoopReact::Exit => return Ok(()),
                    }
                }
                None => {}
            }
        }
        // The capture cap (not silence) ended this clip: say so, so the model
        // does not act on a request that was cut off mid-sentence.
        let text = if cut_by_cap {
            turn_trace.mark("utterance_cut_by_cap", None);
            format!(
                "{text} [host note: the recording reached its {}-second limit while the user was still speaking; this request may be cut off mid-sentence]",
                crate::hear::max_utterance_secs(capture_kind)
            )
        } else {
            text
        };

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
        let mut tts_slot = models::TtsSlot::Loading(rt.tts_loader.load(rt.tts));
        // Streamed answer sentences and tool progress reach the engine thread
        // through this channel; the subscriber only sends, never blocks.
        let (feed_tx, mut speech_feed) = early_speech::SpeechFeed::channel();
        let mut early = EarlySpeech::default();
        rt.agent.set_turn_id(Some(turn.to_string()));
        if let Some(ref sid) = *sess.active_session {
            rt.agent.set_session_id(Some(sid.to_string()));
        }

        // Agent events and the engine publish through the same current status.
        // A later tool event therefore inherits fresh phase, device, input,
        // artifact, and fault fields instead of replaying a turn-start copy.
        let activity_tx = rt.picture.status_tx.clone();
        let activity_latest = rt.picture.latest.clone();
        let activity_turn = turn.to_string();
        let activity_events_enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let activity_events_enabled_w = activity_events_enabled.clone();
        let wave = std::sync::Arc::new(std::sync::Mutex::new(boris_agent::ActivityWave::default()));
        let wave_w = wave.clone();
        let parallel_active = std::sync::atomic::AtomicBool::new(false);
        let thought_t = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
        let thought_w = thought_t.clone();
        let art_store = rt.store.clone();
        let art_sid = (*sess.active_session).clone();
        // Keep the listener for the whole turn — including HITL resume —
        // so post-confirm tools / subagents still update the UI.
        let unsub = rt.agent.subscribe(move |ev| {
            // Speech and progress signals bypass the UI gate below: a barge-in
            // capture pauses UI repaint, not the speech queue.
            let feed_msg = match ev {
                AgentEvent::Speech { signal } => {
                    let _ = feed_tx.send(early_speech::FeedMsg::Speech(signal.clone()));
                    return;
                }
                AgentEvent::ToolExecutionStart { .. } => Some(early_speech::FeedMsg::ToolStarted),
                AgentEvent::ToolNote { text } => {
                    Some(early_speech::FeedMsg::ToolNote(text.clone()))
                }
                AgentEvent::NeedsConfirmation { .. } | AgentEvent::NeedsInput { .. } => {
                    Some(early_speech::FeedMsg::Approval)
                }
                _ => None,
            };
            if let Some(msg) = feed_msg {
                let _ = feed_tx.send(msg);
            }
            if !activity_events_enabled_w.load(std::sync::atomic::Ordering::Acquire) {
                return;
            }
            match ev {
                AgentEvent::ToolBatchStart { .. } => {
                    parallel_active.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                AgentEvent::ToolBatchEnd { .. }
                | AgentEvent::TurnStart { .. }
                | AgentEvent::NeedsConfirmation { .. }
                | AgentEvent::NeedsInput { .. } => {
                    parallel_active.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                _ => {}
            }
            // Track consecutive tool starts for same-kind collapse.
            if let AgentEvent::ToolExecutionStart {
                tool_name,
                args_summary,
                ..
            } = ev
            {
                if let Ok(mut w) = wave_w.lock() {
                    note_tool_start(&mut w, tool_name, args_summary);
                }
                if let Ok(mut t) = thought_w.lock() {
                    *t = std::time::Instant::now();
                }
            }
            if let AgentEvent::TurnStart { .. } = ev {
                if let Ok(mut t) = thought_w.lock() {
                    *t = std::time::Instant::now();
                }
                if let Ok(mut w) = wave_w.lock() {
                    w.reset();
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
                            if let Ok(mut latest) = activity_latest.lock() {
                                if latest.turn.as_deref() == Some(activity_turn.as_str()) {
                                    latest.artifact = Some(peek);
                                    latest.fallback_report = None;
                                }
                            }
                        }
                    }
                }
            }
            if let AgentEvent::ToolNote { text } = ev {
                let Ok(mut latest) = activity_latest.lock() else {
                    return;
                };
                if latest.turn.as_deref() != Some(activity_turn.as_str()) {
                    return;
                }
                latest.thinking = Some(picture::truncate_thinking(text));
                picture::send_locked(&mut latest, &activity_tx);
                return;
            }
            if let AgentEvent::ReportFallback { .. } = ev {
                let Ok(mut latest) = activity_latest.lock() else {
                    return;
                };
                if artifact::apply_report_fallback(&mut latest, &activity_turn, ev) {
                    picture::send_locked(&mut latest, &activity_tx);
                }
                return;
            }
            if let AgentEvent::Reasoning { preview } = ev {
                let Ok(mut latest) = activity_latest.lock() else {
                    return;
                };
                if latest.turn.as_deref() != Some(activity_turn.as_str()) {
                    return;
                }
                // The agent sends a rolling tail. Keep its newest characters
                // when narrowing the status payload for the overlay.
                latest.thinking = Some(picture::truncate_thinking(preview));
                let elapsed = thought_w
                    .lock()
                    .ok()
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                let thought = boris_agent::describe_thought(elapsed);
                // Don't clobber a live tool chip with the think timer.
                let busy_tool = latest.activity.as_deref().is_some_and(is_tool_chip);
                if !busy_tool {
                    latest.activity = Some(format!("thinking · {thought}"));
                } else if latest.activity.is_none() {
                    latest.activity = Some("thinking…".into());
                }
                picture::send_locked(&mut latest, &activity_tx);
                return;
            }
            // Confirmation and typed input replace the current note.
            if matches!(
                ev,
                AgentEvent::NeedsConfirmation { .. } | AgentEvent::NeedsInput { .. }
            ) {
                if let Ok(mut latest) = activity_latest.lock() {
                    if latest.turn.as_deref() == Some(activity_turn.as_str()) {
                        latest.thinking = None;
                    }
                }
            }
            let wave_snapshot = wave_w.lock().ok().map(|g| g.clone()).unwrap_or_default();
            // Keep the explicit parallel label while individual child tools run.
            if parallel_active.load(std::sync::atomic::Ordering::Relaxed)
                && matches!(
                    ev,
                    AgentEvent::ToolExecutionStart { .. }
                        | AgentEvent::ToolExecutionEnd { .. }
                        | AgentEvent::ToolProgress { .. }
                )
            {
                return;
            }
            let Some(label) = activity_label(ev, &wave_snapshot) else {
                return;
            };
            let Ok(mut latest) = activity_latest.lock() else {
                return;
            };
            if latest.turn.as_deref() != Some(activity_turn.as_str()) {
                return;
            }
            latest.activity = Some(label);
            picture::send_locked(&mut latest, &activity_tx);
        });

        let work = match interrupted_text.as_deref() {
            Some(previous) => AgentWork::Replacement {
                user_text: &text,
                interrupted_text: Some(previous),
            },
            None => AgentWork::Prompt(&text),
        };
        // Cancellation is cooperative. On a barge-in stop or replacement the
        // agent abandons the turn against this pre-turn checkpoint: the
        // undelivered answer never leaks into the next model request, while
        // tools that already executed stay visible (with a host note) so
        // their side effects are not redone or contradicted. A turn where
        // nothing executed is restored to this checkpoint exactly.
        let turn_checkpoint = rt.agent.checkpoint();
        let mut think_trace_ms = turn_trace.elapsed_ms();
        // Agent time (model + tools) and audio played during Thinking, over
        // the first pass and any confirm/input resumes.
        let mut think_timing = ThinkTiming::default();
        let think = run_thinking(ThinkCtx {
            agent: &mut rt.agent,
            agent_rt: &rt.agent_rt,
            mic: &rt.mic,
            wake: &mut rt.wake,
            vad: &mut rt.vad,
            stt: &mut rt.stt,
            liveness: &mut rt.liveness,
            barge_in: rt.barge_in,
            audio: &mut rt.audio,
            output_events: &mut rt.output_events,
            picture: &mut rt.picture,
            activity_events_enabled: activity_events_enabled.clone(),
            cmd_rx: &cmd_rx,
            running: &mut running,
            work,
            turn,
            speech: Some(ThinkSpeech {
                feed: &mut speech_feed,
                tts: &mut tts_slot,
                progress_line: rt.progress_line,
                report: &mut early,
            }),
            timing: &mut think_timing,
        });
        let (tts_owned, tts_load) = tts_slot.join();
        rt.tts = tts_owned;
        trace_early_audio(&mut turn_trace, &early, think_trace_ms);
        turn_trace.set_agent_timing(think_timing.agent_ms, think_timing.playback_ms);

        let (outcome, report) = match think {
            ThinkResolve::Stopped => {
                turn_trace.mark("thinking_stopped", None);
                unsub();
                rt.agent.abort();
                go_off_session(&mut rt, &mut sess);
                continue;
            }
            ThinkResolve::StopTurn => {
                turn_trace.mark("thinking_barge_stop", None);
                unsub();
                rt.agent.abandon_turn(turn_checkpoint.clone());
                rt.picture.clear_activity();
                maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
                follow_up_depth = 0;
                await_reply = false;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
            ThinkResolve::TakeTurn(next) => {
                let interrupted_text = rt.picture.heard.clone();
                turn_trace.mark(
                    "thinking_barge_take_turn",
                    Some(serde_json::json!({ "text": next })),
                );
                unsub();
                rt.agent.abandon_turn(turn_checkpoint.clone());
                rt.picture.clear_activity();
                pending_barge_turn = Some(PendingBargeTurn {
                    user_text: next,
                    interrupted_text,
                });
                follow_up_depth = 0;
                await_reply = false;
                continue;
            }
            ThinkResolve::Finished(Ok(pair)) => pair,
            ThinkResolve::Finished(Err(e)) if e.kind() == AgentErrorKind::Cancelled => {
                turn_trace.mark("thinking_cancelled", None);
                unsub();
                rt.agent.abort();
                rt.picture.clear_activity();
                maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
                follow_up_depth = 0;
                await_reply = false;
                rt.picture.set_phase(Phase::Armed);
                continue;
            }
            ThinkResolve::Finished(Err(e)) => {
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
                let spoken = speak_host_line(
                    &mut rt,
                    &cmd_rx,
                    &mut running,
                    "I glitched mid-task. Wake me and say continue if you want me to finish.",
                    turn,
                );
                follow_up_depth = 0;
                rt.picture.set_phase(Phase::Armed);
                match after_host_line(
                    &mut rt,
                    &mut sess,
                    spoken.next,
                    running,
                    &mut pending_barge_turn,
                    &mut await_reply,
                    &mut follow_up_depth,
                ) {
                    LoopReact::Continue => continue,
                    LoopReact::Exit => return Ok(()),
                }
            }
        };
        // Live tool labels already mirrored; keep a soft chip until speech/confirm.
        if !report.tools_used.is_empty() {
            rt.picture.activity = Some(boris_agent::summarize_tools_used(&report.tools_used));
        }
        rt.picture.sync_fallback_report();
        if report.tools_used.iter().any(|n| n == "present_artifact")
            && rt.picture.fallback_report.is_none()
        {
            if let Some(sid) = sess.active_session.as_ref() {
                match peek_current(&rt.store, sid) {
                    Some(peek) => rt.picture.artifact = Some(peek),
                    None => tracing::debug!(
                        %turn,
                        "present_artifact ran but index peek missed (flush race?)"
                    ),
                }
            }
        }
        if !report.tools_used.is_empty() || rt.picture.artifact.is_some() {
            rt.picture.publish();
        }
        rt.picture.update_context(
            report.context_used_tokens,
            report.context_limit_tokens,
            report.context_estimated,
        );

        // Resolve HITL confirmations (voice yes/no) before final speech.
        let original_heard = rt.picture.heard.clone();
        let max_confirms = rt.max_confirms_per_turn;
        let mut confirm = ConfirmCtx {
            agent: &mut rt.agent,
            agent_rt: &rt.agent_rt,
            tts: &mut rt.tts,
            stt: &mut rt.stt,
            mic: &rt.mic,
            wake: &mut rt.wake,
            vad: &mut rt.vad,
            liveness: &mut rt.liveness,
            barge_in: rt.barge_in,
            audio: &mut rt.audio,
            output_events: &mut rt.output_events,
            cmd_rx: &cmd_rx,
            running: &mut running,
            picture: &mut rt.picture,
            store: &rt.store,
            active_session: sess.active_session,
            transcript_len: sess.transcript_len,
            original_heard,
            activity_events_enabled: activity_events_enabled.clone(),
            turn_checkpoint,
            turn,
            max_confirms,
            residency: rt.residency,
            last_resume_report: None,
            feed: &mut speech_feed,
            progress_line: rt.progress_line,
            resume_speech: Vec::new(),
            timing: ThinkTiming::default(),
        };
        let resolved = resolve_agent_outcome(outcome, &mut confirm);
        think_timing.add(confirm.timing);
        turn_trace.set_agent_timing(think_timing.agent_ms, think_timing.playback_ms);
        // Resumes stream like the first pass. The last one produced the final
        // reply, so its early speech replaces the first pass's.
        for pass in std::mem::take(&mut confirm.resume_speech) {
            let base_ms = pass
                .started_at
                .map_or(think_trace_ms, |at| turn_trace.ms_at(at));
            trace_early_audio(&mut turn_trace, &pass, base_ms);
            think_trace_ms = base_ms;
            early = pass;
        }
        let outcome = match resolved {
            OutcomeResolve::Stopped => {
                unsub();
                continue;
            }
            OutcomeResolve::ReArm => {
                unsub();
                follow_up_depth = 0;
                continue;
            }
            OutcomeResolve::TakeTurn {
                user_text,
                interrupted_text,
            } => {
                turn_trace.mark(
                    "confirm_barge_take_turn",
                    Some(serde_json::json!({ "text": user_text })),
                );
                unsub();
                rt.agent.abort();
                rt.picture.clear_activity();
                pending_barge_turn = Some(PendingBargeTurn {
                    user_text,
                    interrupted_text,
                });
                follow_up_depth = 0;
                await_reply = false;
                continue;
            }
            OutcomeResolve::Done(done) => {
                let ResolveDone {
                    outcome,
                    resume_report,
                } = *done;
                // A confirm/input resume ran more tool rounds after the initial
                // report: refresh the meter (already updated inside the resume)
                // and fold the resume rounds into the trace so they are not
                // invisible to latency/tool accounting.
                if let Some(resume) = resume_report {
                    rt.picture.sync_fallback_report();
                    rt.picture.update_context(
                        resume.context_used_tokens,
                        resume.context_limit_tokens,
                        resume.context_estimated,
                    );
                    // A card presented during the resume must reach the overlay
                    // too — the pre-confirm peek above predates those tools.
                    if resume.tools_used.iter().any(|n| n == "present_artifact")
                        && rt.picture.fallback_report.is_none()
                    {
                        if let Some(sid) = sess.active_session.as_ref() {
                            match peek_current(&rt.store, sid) {
                                Some(peek) => rt.picture.artifact = Some(peek),
                                None => tracing::debug!(
                                    %turn,
                                    "resume presented artifact but index peek missed"
                                ),
                            }
                        }
                    }
                    turn_trace.span(
                        "agent_resume",
                        resume.duration.as_millis() as u64,
                        Some(serde_json::json!({
                            "tool_rounds": resume.tool_rounds,
                            "tools": resume.tools_used,
                        })),
                    );
                }
                outcome
            }
        };
        unsub(); // full turn finished (or speech path next)

        // Model + tools only: audio played during Thinking and the confirm
        // dialog are not agent latency.
        let agent_ms = think_timing.agent_ms;
        turn_trace.span(
            "agent",
            agent_ms,
            Some(serde_json::json!({
                "tool_rounds": report.tool_rounds,
                "tools": report.tools_used,
            })),
        );
        turn_trace.span("thinking_playback", think_timing.playback_ms, None);
        tracing::info!(
            %turn,
            agent_ms,
            thinking_playback_ms = think_timing.playback_ms,
            wall_ms = agent_t.elapsed().as_millis() as u64,
            "agent done"
        );

        // The model chose silence (speech not meant for Boris). Tool work
        // never reaches here as Silent: the agent always reports it.
        if matches!(outcome, AgentOutcome::Silent) && report.tools_used.is_empty() {
            turn_trace.mark("agent_silent", None);
            tracing::info!(%turn, "agent chose silence; nothing spoken");
            if let Some(ref sid) = *sess.active_session {
                enqueue_transcript_sync(&rt.store, sid, sess.transcript_len, &rt.agent);
            }
            rt.picture.clear_activity();
            maybe_unload_tts(rt.tts.as_mut(), turn, rt.residency);
            follow_up_depth = 0;
            await_reply = false;
            rt.picture.set_phase(Phase::Armed);
            continue;
        }

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
            AgentOutcome::NeedsInput { .. } => {
                tracing::warn!(%turn, "unresolved input after resolve pass");
                (
                    "I needed you to type something and lost the thread. Wake me and try again."
                        .to_string(),
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
            // The reply was already published as `said` above, but it will
            // never be spoken — clear it so the overlay doesn't show text
            // the user never heard.
            rt.picture.said = None;
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
        // Sentences a cancelled or aborted streamed answer already played to
        // the end are not said twice when the reply starts with them.
        let mut nothing_left = false;
        if early.delivered.is_none() && !early.spoken.is_empty() {
            let skip = early_speech::skip_spoken_prefix(&units, &early.spoken);
            if skip > 0 {
                tracing::info!(%turn, skip, total = units.len(), "skipping already spoken sentences");
                turn_trace.mark(
                    "answer_prefix_skipped",
                    Some(serde_json::json!({ "units": skip, "of": units.len() })),
                );
                units.drain(..skip);
                nothing_left = units.is_empty();
            }
        }

        // Decide follow-up before playback. Balanced and low-latency retain
        // both models across an active follow-up chain; low-memory reloads STT
        // while the next utterance is captured.
        let will_await = expect_reply && follow_up_depth < MAX_FOLLOW_UPS;
        let mut speech_trace_start_ms = turn_trace.elapsed_ms();
        let mut already_audible = false;
        let mut barge_terminal = false;
        let mut speech;
        // The committed answer already played while the agent wrapped up.
        if let Some(done) = early.delivered.take() {
            speech_trace_start_ms = think_trace_ms.saturating_add(early.delivered_after_ms);
            turn_trace.mark_at("answer_streamed", speech_trace_start_ms, None, None);
            speech = done;
        } else if nothing_left {
            // The whole reply was already heard during Thinking.
            speech = speech::StreamedSpeech::nothing_left(lost_tts());
        } else {
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
                    speech::SpeechUnits::Fixed(units),
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
                        // `heard` still holds this turn's original utterance:
                        // speech never overwrites it. Carry it so the next turn's
                        // replacement quotes the real request, not nothing.
                        let interrupted_text = rt.picture.heard.clone();
                        pending_barge_turn = Some(PendingBargeTurn {
                            user_text: text,
                            interrupted_text,
                        });
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
        if pending_barge_turn.is_some() || barge_terminal {
            continue;
        }
        match speech.wait {
            PlaybackWait::Stopped => {
                turn_trace.mark("audio_stopped", None);
                go_off_session(&mut rt, &mut sess);
                continue;
            }
            PlaybackWait::Aborted | PlaybackWait::BargedIn => {
                // `BargedIn` is consumed inside the speak loop above and never
                // reaches here; the joint arm keeps the match total without a
                // production `unreachable!` if that ever changes.
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
    fn reminders_are_spoken_only_in_a_gap_between_turns() {
        assert!(reminder_gap_open(Phase::Armed, false, false));
        assert!(
            !reminder_gap_open(Phase::Armed, true, false),
            "pending confirm"
        );
        assert!(
            !reminder_gap_open(Phase::Armed, false, true),
            "queued barge turn"
        );
        for phase in [
            Phase::Off,
            Phase::Quiet,
            // The user may already be answering: never talk over them.
            Phase::AwaitingReply,
            Phase::Hearing,
            Phase::Reading,
            Phase::Thinking,
            Phase::Talking,
            Phase::AwaitingConfirm,
            Phase::AwaitingInput,
        ] {
            assert!(!reminder_gap_open(phase, false, false), "{phase:?}");
        }
    }

    #[test]
    fn a_due_reminder_waits_out_the_turn_and_is_taken_at_the_next_gap() {
        let now = boris_agent::now_unix_ms();
        let path = std::env::temp_dir().join(format!(
            "boris-engine-reminder-gap-{}-{now}.json",
            std::process::id()
        ));
        // Written before the store opens, so the item is already due.
        let file = serde_json::json!({
            "next_id": 2,
            "items": [{
                "id": 1,
                "kind": "reminder",
                "message": "call mum",
                "due_unix_ms": now - 1_000,
                "created_unix_ms": now - 60_000,
            }],
        });
        std::fs::write(&path, file.to_string()).unwrap();
        let store = boris_agent::ReminderStore::open(&path);

        for phase in [
            Phase::AwaitingReply,
            Phase::Hearing,
            Phase::Reading,
            Phase::Thinking,
            Phase::Talking,
            Phase::AwaitingConfirm,
            Phase::AwaitingInput,
        ] {
            let gap = reminder_gap_open(phase, false, false);
            assert!(due_at_gap(&store, gap, now).is_empty(), "{phase:?}");
            assert!(store.has_due(now), "still due after {phase:?}");
        }
        // Armed with an approval still pending: not yet, and the Armed wait
        // does not yield for it (no busy loop).
        let gap = reminder_gap_open(Phase::Armed, true, false);
        assert!(due_at_gap(&store, gap, now).is_empty());
        assert!(!reminders_ready(gap, None, store.has_due(now)));

        // Armed: offered, but only removed once the line was heard.
        let gap = reminder_gap_open(Phase::Armed, false, false);
        assert!(reminders_ready(gap, None, store.has_due(now)));
        let due = due_at_gap(&store, gap, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].spoken(now), "Reminder: call mum.");
        assert!(store.has_due(now), "peeking does not remove");

        // Not heard (TTS/playback failed): kept, and the wait backs off.
        let mut retry_at = None;
        assert!(settle_reminders(&store, false, now, &mut retry_at).is_empty());
        assert!(store.has_due(now));
        assert!(retry_at.is_some());
        assert!(!reminders_ready(gap, retry_at, store.has_due(now)));
        let past = std::time::Instant::now() - std::time::Duration::from_secs(1);
        assert!(reminders_ready(gap, Some(past), store.has_due(now)));

        // Heard: removed exactly once and the backoff clears.
        let taken = settle_reminders(&store, true, now, &mut retry_at);
        assert_eq!(taken, due);
        assert!(retry_at.is_none());
        assert!(!store.has_due(now));
        assert!(due_at_gap(&store, gap, now).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loop_react_enum_is_copy() {
        let a = LoopReact::Continue;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(LoopReact::Exit, LoopReact::Exit);
    }
}
