//! Speech that starts while the agent is still Thinking: the streamed final
//! answer, and one short progress line on long tool turns.
//!
//! The agent-event subscriber runs on the agent thread and only `send`s into
//! an unbounded channel; the engine thread drains it between polls, so the
//! agent never blocks and the engine stays the sole owner of playback.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use boris_agent::SpeechSignal;

use super::speech::AppendedUnit;
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

/// A unit counts as heard only once the playback clock is this far past its
/// end: output latency means the clock runs slightly ahead of the speaker.
const HEARD_MARGIN_MS: u64 = 250;

/// Which sentences of one streamed answer were played to the end.
///
/// The output device reports no per-unit progress, so this replays each
/// speech call on a playback clock: a unit starts when it was appended or
/// when the audio before it ends (whichever is later, so underruns are not
/// counted as playback), and the clock stops when the call stopped feeding.
/// A barge-in pause keeps the queue, so the next call continues the clock.
#[derive(Debug, Default)]
pub(super) struct SpokenLedger {
    /// Unit text and its end on the playback clock.
    units: Vec<(String, u64)>,
    queued_end_ms: u64,
    played_ms: u64,
    drained: bool,
}

impl SpokenLedger {
    /// Fold in one speech call. `play_origin_ms` is when its playback began
    /// (both relative to the call); `drained` means every queued unit played.
    pub fn record(
        &mut self,
        appended: &[AppendedUnit],
        play_origin_ms: Option<u64>,
        stop_ms: u64,
        drained: bool,
    ) {
        let base = self.played_ms;
        let clock = |t: u64| match play_origin_ms {
            Some(origin) => base.saturating_add(t.saturating_sub(origin)),
            None => base,
        };
        for unit in appended {
            let start = self.queued_end_ms.max(clock(unit.at_ms));
            let end = start.saturating_add(unit.dur_ms);
            self.units.push((unit.text.clone(), end));
            self.queued_end_ms = end;
        }
        self.played_ms = clock(stop_ms).min(self.queued_end_ms).max(base);
        if drained {
            self.played_ms = self.queued_end_ms;
            self.drained = true;
        }
    }

    /// The leading units heard in full, in order.
    pub fn spoken(&self) -> Vec<String> {
        self.units
            .iter()
            .take_while(|(_, end)| {
                self.drained || end.saturating_add(HEARD_MARGIN_MS) <= self.played_ms
            })
            .map(|(text, _)| text.clone())
            .collect()
    }
}

/// How many leading `units` of the final reply the user already heard in
/// full (`spoken`). Compares words case-insensitively, ignoring whitespace,
/// and only skips whole units of the reply; a reply that does not start with
/// the same text skips nothing.
pub(super) fn skip_spoken_prefix(units: &[String], spoken: &[String]) -> usize {
    fn words(text: &str) -> impl Iterator<Item = String> + '_ {
        text.split_whitespace().map(str::to_lowercase)
    }
    let heard: Vec<String> = spoken.iter().flat_map(|unit| words(unit)).collect();
    let mut matched = 0usize;
    let mut skip = 0usize;
    for (i, unit) in units.iter().enumerate() {
        let unit_words: Vec<String> = words(unit).collect();
        let end = matched + unit_words.len();
        if end > heard.len() || heard[matched..end] != unit_words[..] {
            break;
        }
        matched = end;
        if !unit_words.is_empty() {
            skip = i + 1;
        }
    }
    skip
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

    fn unit(text: &str, at_ms: u64, dur_ms: u64) -> AppendedUnit {
        AppendedUnit {
            text: text.into(),
            at_ms,
            dur_ms,
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn skip_spoken_prefix_only_skips_text_the_reply_really_starts_with() {
        let reply = strings(&["Let me check.", "The build passed.", "Anything else?"]);
        assert_eq!(skip_spoken_prefix(&reply, &strings(&["let  me CHECK."])), 1);
        assert_eq!(
            skip_spoken_prefix(&reply, &strings(&["Let me check.", "The build passed."])),
            2
        );
        // Everything already heard.
        assert_eq!(
            skip_spoken_prefix(
                &reply,
                &strings(&["Let me check. The build passed.", "Anything else?"])
            ),
            3
        );
        // Rewritten after the tool call: speak it all.
        assert_eq!(skip_spoken_prefix(&reply, &strings(&["Let me look."])), 0);
        // Heard only part of the reply's first unit: speak it all.
        let one = strings(&["Let me check the build."]);
        assert_eq!(skip_spoken_prefix(&one, &strings(&["Let me check"])), 0);
        assert_eq!(skip_spoken_prefix(&reply, &[]), 0);
        assert_eq!(skip_spoken_prefix(&[], &strings(&["Hi there."])), 0);
    }

    #[test]
    fn ledger_counts_only_units_played_to_the_end() {
        let mut ledger = SpokenLedger::default();
        // Audio starts at 100 ms; units of 1 s each; stopped at 1.6 s.
        ledger.record(
            &[unit("One.", 80, 1_000), unit("Two.", 400, 1_000)],
            Some(100),
            1_600,
            false,
        );
        assert_eq!(ledger.spoken(), strings(&["One."]));

        // Nothing ever became audible.
        let mut silent = SpokenLedger::default();
        silent.record(&[unit("One.", 80, 1_000)], None, 5_000, false);
        assert!(silent.spoken().is_empty());

        // Drained: every queued unit was heard.
        let mut done = SpokenLedger::default();
        done.record(&[unit("One.", 0, 900)], Some(10), 920, true);
        assert_eq!(done.spoken(), strings(&["One."]));
    }

    #[test]
    fn ledger_does_not_count_underruns_as_playback() {
        let mut ledger = SpokenLedger::default();
        // "Two." arrives 1 s after "One." finished, so at 2.4 s it has played
        // for only 0.4 of its 1.0 s.
        ledger.record(
            &[unit("One.", 0, 1_000), unit("Two.", 2_000, 1_000)],
            Some(0),
            2_400,
            false,
        );
        assert_eq!(ledger.spoken(), strings(&["One."]));
    }

    #[test]
    fn ledger_continues_across_a_barge_in_pause() {
        let mut ledger = SpokenLedger::default();
        // Paused 500 ms into "Two." (queued but not finished).
        ledger.record(
            &[unit("One.", 0, 1_000), unit("Two.", 100, 1_000)],
            Some(0),
            1_500,
            false,
        );
        assert_eq!(ledger.spoken(), strings(&["One."]));
        // Resume: leftover "Two." plays first, then "Three." is appended.
        ledger.record(&[unit("Three.", 50, 1_000)], Some(0), 1_000, false);
        assert_eq!(ledger.spoken(), strings(&["One.", "Two."]));
    }

    #[test]
    fn cancelled_stream_then_final_reply_speaks_only_the_rest() {
        // Streamed through the feed, then a late Cancel stopped playback
        // halfway through the second sentence.
        let (tx, mut feed) = feed();
        for chunk in ["Sure thing.", "Checking the build now."] {
            tx.send(FeedMsg::Speech(SpeechSignal::Chunk(chunk.into())))
                .unwrap();
        }
        feed.drain();
        let first = feed.next_unit().unwrap();
        let second = feed.next_unit().unwrap();
        let mut ledger = SpokenLedger::default();
        ledger.record(
            &[unit(&first, 0, 800), unit(&second, 300, 1_200)],
            Some(50),
            1_500,
            false,
        );
        let spoken = ledger.spoken();
        assert_eq!(spoken, vec![first.clone()]);

        let reply = speakable_reply_units("Sure thing. Checking the build now. It passed.");
        let skip = skip_spoken_prefix(&reply, &spoken);
        // Without sentence segmentation the reply is one unit that was not
        // heard in full, so nothing is skipped.
        assert_eq!(skip, usize::from(reply.first() == Some(&first)));
        #[cfg(feature = "tts-supertone")]
        assert_eq!(skip, 1);
        assert!(!reply[skip..].is_empty());
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
