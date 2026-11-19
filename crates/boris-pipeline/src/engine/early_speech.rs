//! Speech that starts while the agent is still Thinking: the streamed final
//! answer, and one short progress line on long tool turns.
//!
//! The agent-event subscriber runs on the agent thread and only `send`s into
//! an unbounded channel; the engine thread drains it between polls, so the
//! agent never blocks and the engine stays the sole owner of playback.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use boris_agent::SpeechSignal;

use super::util::speakable_reply_units;

/// Tool work must run this long before the progress line is spoken.
pub(super) const PROGRESS_LINE_AFTER: Duration = Duration::from_secs(7);
/// Spoken when the model left no usable progress note.
pub(super) const PROGRESS_FALLBACK: &str = "Still working on it.";
const PROGRESS_MAX_CHARS: usize = 120;

/// Messages from the agent-event subscriber to the engine thread.
#[derive(Debug, Clone)]
pub(super) enum FeedMsg {
    Speech(SpeechSignal),
    ToolStarted,
    ToolNote(String),
    Approval,
}

/// Engine-side state of one turn's early speech.
pub(super) struct SpeechFeed {
    rx: crossbeam_channel::Receiver<FeedMsg>,
    pending: VecDeque<String>,
    committed: bool,
    cancelled: bool,
    /// Set after a delivered, failed, or interrupted early job: anything
    /// later is ignored and the reply goes through the normal speech path.
    disabled: bool,
    said: String,
    tool_started: Option<Instant>,
    note: Option<String>,
    approval: bool,
}

impl SpeechFeed {
    pub fn channel() -> (crossbeam_channel::Sender<FeedMsg>, Self) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (
            tx,
            Self {
                rx,
                pending: VecDeque::new(),
                committed: false,
                cancelled: false,
                disabled: false,
                said: String::new(),
                tool_started: None,
                note: None,
                approval: false,
            },
        )
    }

    /// Apply every queued message. Returns true when new speech arrived.
    pub fn drain(&mut self) -> bool {
        let mut added = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                FeedMsg::Speech(SpeechSignal::Chunk(text)) => {
                    if self.disabled {
                        continue;
                    }
                    if !self.said.is_empty() {
                        self.said.push(' ');
                    }
                    self.said.push_str(&text);
                    self.pending.extend(speakable_reply_units(&text));
                    added = true;
                }
                FeedMsg::Speech(SpeechSignal::Cancel) => {
                    self.pending.clear();
                    self.said.clear();
                    self.committed = false;
                    self.cancelled = true;
                }
                FeedMsg::Speech(SpeechSignal::Commit) => {
                    if !self.disabled {
                        self.committed = true;
                    }
                }
                FeedMsg::ToolStarted => {
                    self.tool_started.get_or_insert_with(Instant::now);
                }
                FeedMsg::ToolNote(text) => self.note = Some(text),
                FeedMsg::Approval => self.approval = true,
            }
        }
        added
    }

    pub fn has_units(&self) -> bool {
        !self.disabled && !self.pending.is_empty()
    }

    pub fn next_unit(&mut self) -> Option<String> {
        if self.disabled {
            return None;
        }
        self.pending.pop_front()
    }

    /// Put units that were not played (barge-in pause) back in front.
    pub fn requeue(&mut self, units: Vec<String>) {
        for unit in units.into_iter().rev() {
            self.pending.push_front(unit);
        }
    }

    /// True once since the last call if a Cancel arrived.
    pub fn take_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.cancelled)
    }

    pub fn committed(&self) -> bool {
        self.committed && !self.disabled
    }

    pub fn disable(&mut self) {
        self.disabled = true;
        self.pending.clear();
    }

    pub fn said(&self) -> &str {
        &self.said
    }

    /// Tool work has run long enough for one spoken progress line, and no
    /// approval or typed-input prompt is involved.
    pub fn progress_due(&self, now: Instant) -> bool {
        !self.approval
            && self
                .tool_started
                .is_some_and(|at| now.saturating_duration_since(at) >= PROGRESS_LINE_AFTER)
    }

    /// The model's latest progress note, sanitized and short, or a fixed line.
    pub fn progress_text(&self) -> String {
        self.note
            .as_deref()
            .and_then(short_progress_line)
            .unwrap_or_else(|| PROGRESS_FALLBACK.to_string())
    }
}

/// First sentence of a progress note, without tool markup, capped at a word
/// boundary. `None` when nothing speakable is left.
pub(super) fn short_progress_line(note: &str) -> Option<String> {
    let cleaned = boris_agent::strip_tool_markup(note);
    let first = speakable_reply_units(&cleaned).into_iter().next()?;
    let first = first.trim();
    if first.split_whitespace().count() < 2 || first.contains("```") {
        return None;
    }
    if first.chars().count() <= PROGRESS_MAX_CHARS {
        return Some(first.to_string());
    }
    let mut out = String::new();
    for word in first.split_whitespace() {
        if out.chars().count() + word.chars().count() + 1 > PROGRESS_MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let out = out.trim_end_matches([',', ';', ':']).to_string();
    (!out.is_empty()).then(|| format!("{out}…"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed() -> (crossbeam_channel::Sender<FeedMsg>, SpeechFeed) {
        SpeechFeed::channel()
    }

    #[test]
    fn late_cancel_drops_queued_units_and_flags_the_job() {
        let (tx, mut feed) = feed();
        tx.send(FeedMsg::Speech(SpeechSignal::Chunk("Let me check.".into())))
            .unwrap();
        assert!(feed.drain());
        assert!(feed.has_units());
        tx.send(FeedMsg::Speech(SpeechSignal::Cancel)).unwrap();
        feed.drain();
        assert!(!feed.has_units());
        assert!(feed.said().is_empty());
        assert!(feed.take_cancelled());
        assert!(!feed.take_cancelled(), "flag is consumed");
    }

    #[test]
    fn cancel_then_new_answer_in_one_drain_keeps_the_new_answer() {
        let (tx, mut feed) = feed();
        for msg in [
            FeedMsg::Speech(SpeechSignal::Chunk("Progress note.".into())),
            FeedMsg::Speech(SpeechSignal::Cancel),
            FeedMsg::Speech(SpeechSignal::Chunk("The answer is four.".into())),
            FeedMsg::Speech(SpeechSignal::Commit),
        ] {
            tx.send(msg).unwrap();
        }
        feed.drain();
        assert!(feed.take_cancelled());
        assert!(feed.committed());
        assert_eq!(feed.said(), "The answer is four.");
        assert_eq!(feed.next_unit().as_deref(), Some("The answer is four."));
    }

    #[test]
    fn disabled_feed_ignores_later_speech() {
        let (tx, mut feed) = feed();
        feed.disable();
        tx.send(FeedMsg::Speech(SpeechSignal::Chunk(
            "Late words here.".into(),
        )))
        .unwrap();
        tx.send(FeedMsg::Speech(SpeechSignal::Commit)).unwrap();
        feed.drain();
        assert!(!feed.has_units());
        assert!(!feed.committed());
    }

    #[test]
    fn progress_line_waits_for_tool_time_and_never_during_approval() {
        let (tx, mut feed) = feed();
        let now = Instant::now();
        assert!(!feed.progress_due(now + Duration::from_secs(60)));
        tx.send(FeedMsg::ToolStarted).unwrap();
        feed.drain();
        assert!(!feed.progress_due(Instant::now()));
        assert!(feed.progress_due(Instant::now() + PROGRESS_LINE_AFTER));
        tx.send(FeedMsg::Approval).unwrap();
        feed.drain();
        assert!(!feed.progress_due(Instant::now() + PROGRESS_LINE_AFTER));
    }

    #[test]
    fn progress_text_prefers_a_short_sanitized_note() {
        let (tx, mut feed) = feed();
        assert_eq!(feed.progress_text(), PROGRESS_FALLBACK);
        tx.send(FeedMsg::ToolNote(
            "I'll scan the logs for the crash. Then I'll summarize.".into(),
        ))
        .unwrap();
        feed.drain();
        assert_eq!(feed.progress_text(), "I'll scan the logs for the crash.");
        assert_eq!(short_progress_line("ok"), None);
        assert_eq!(short_progress_line("<tool_call>{}</tool_call>"), None);
        let long = "word ".repeat(60);
        let line = short_progress_line(&long).unwrap();
        assert!(line.chars().count() <= PROGRESS_MAX_CHARS + 1);
        assert!(line.ends_with('…'));
    }
}
