//! Stream the final reply into speech sentence by sentence while the model is
//! still generating it.
//!
//! Content sent alongside tool calls is an on-screen progress note and must
//! never be spoken. In SSE, tool-call deltas can arrive *after* that content,
//! so the first sentence is held until the stream has run [`DEFAULT_RELEASE_GRACE`]
//! past its first sentence boundary without a tool call starting, or until
//! the response ends. A tool call that starts after release cancels the
//! response's queued speech ([`SpeechSignal::Cancel`]).
//!
//! The loop owns one [`SpeechSegmenter`] per model response. When it accepts
//! a reply as the final answer it calls [`SpeechSegmenter::finish`], which
//! emits whatever was not streamed yet followed by [`SpeechSignal::Commit`], or
//! a `Cancel` when the accepted text no longer starts with what was streamed
//! (the host then speaks the reply itself). Every chunk goes through the
//! speech sanitizer, and a reply that could be `[silent]` or tool markup is
//! never streamed early.

use std::time::{Duration, Instant};

/// How long the stream must run past the first sentence boundary, with no
/// tool call started, before that sentence may be spoken.
pub const DEFAULT_RELEASE_GRACE: Duration = Duration::from_millis(900);

/// One instruction for the host's speech queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechSignal {
    /// A sanitized sentence (or the remaining tail) ready for synthesis.
    Chunk(String),
    /// Drop every chunk of the current response that is queued or playing.
    Cancel,
    /// The chunks so far are the whole accepted final answer.
    Commit,
}

/// Incremental sentence segmenter for one model response.
#[derive(Debug)]
pub struct SpeechSegmenter {
    grace: Duration,
    buf: String,
    /// Bytes of `buf` already split into sentences.
    cut: usize,
    /// Raw sentences waiting for release.
    held: Vec<String>,
    /// Raw text already emitted as chunks (for the finish prefix check).
    emitted_raw: String,
    emitted: usize,
    first_boundary: Option<Instant>,
    released: bool,
    tool_seen: bool,
}

impl Default for SpeechSegmenter {
    fn default() -> Self {
        Self::new(DEFAULT_RELEASE_GRACE)
    }
}

impl SpeechSegmenter {
    pub fn new(grace: Duration) -> Self {
        Self {
            grace,
            buf: String::new(),
            cut: 0,
            held: Vec::new(),
            emitted_raw: String::new(),
            emitted: 0,
            first_boundary: None,
            released: false,
            tool_seen: false,
        }
    }

    /// Whether any chunk of this response reached the host.
    pub fn has_emitted(&self) -> bool {
        self.emitted > 0
    }

    /// Start a new response. Cancels anything already streamed from the old one.
    pub fn reset(&mut self) -> Vec<SpeechSignal> {
        let out = self.cancel_emitted();
        *self = Self::new(self.grace);
        out
    }

    /// Feed one content delta.
    pub fn push_content(&mut self, delta: &str, now: Instant) -> Vec<SpeechSignal> {
        if self.tool_seen || delta.is_empty() {
            return Vec::new();
        }
        self.buf.push_str(delta);
        while let Some(end) = next_boundary(&self.buf, self.cut) {
            let raw = self.buf[self.cut..end].to_string();
            self.cut = end;
            self.first_boundary.get_or_insert(now);
            self.held.push(raw);
        }
        self.poll(now)
    }

    /// A tool-call delta arrived: this response is not the final answer.
    pub fn on_tool_call(&mut self) -> Vec<SpeechSignal> {
        if self.tool_seen {
            return Vec::new();
        }
        self.tool_seen = true;
        self.held.clear();
        self.cancel_emitted()
    }

    /// Release held sentences once the grace period has passed.
    pub fn poll(&mut self, now: Instant) -> Vec<SpeechSignal> {
        if self.tool_seen || self.hold_for_finish() {
            return Vec::new();
        }
        if !self.released {
            let Some(at) = self.first_boundary else {
                return Vec::new();
            };
            if now.saturating_duration_since(at) < self.grace {
                return Vec::new();
            }
            self.released = true;
        }
        let held = std::mem::take(&mut self.held);
        let mut out = Vec::new();
        for raw in held {
            self.emitted_raw.push_str(&raw);
            if let Some(chunk) = sanitize_chunk(&raw) {
                self.emitted += 1;
                out.push(SpeechSignal::Chunk(chunk));
            }
        }
        out
    }

    /// The loop accepted `accepted` as the final answer of this response.
    pub fn finish(&mut self, accepted: &str) -> Vec<SpeechSignal> {
        let accepted = accepted.trim();
        if accepted.is_empty() || crate::outcome::is_silence_reply(accepted) {
            return self.reset();
        }
        let rest = if self.emitted == 0 {
            Some(accepted)
        } else {
            strip_prefix_ignoring_ws(accepted, &self.emitted_raw)
        };
        let Some(rest) = rest else {
            // The accepted text diverged from what was streamed (for example a
            // fallback reply). The host speaks the full reply instead.
            return self.reset();
        };
        let mut out = Vec::new();
        if let Some(chunk) = sanitize_chunk(rest) {
            out.push(SpeechSignal::Chunk(chunk));
        }
        if self.emitted > 0 || !out.is_empty() {
            out.push(SpeechSignal::Commit);
        }
        *self = Self::new(self.grace);
        out
    }

    fn cancel_emitted(&mut self) -> Vec<SpeechSignal> {
        if self.emitted == 0 {
            return Vec::new();
        }
        self.emitted = 0;
        self.emitted_raw.clear();
        vec![SpeechSignal::Cancel]
    }

    /// Text that might be `[silent]`, tool markup, JSON, or code is never
    /// released early; the finish path decides once the reply is accepted.
    fn hold_for_finish(&self) -> bool {
        let head = self.buf.trim_start();
        head.starts_with(['[', '{', '<', '`'])
            || self.buf.contains("```")
            || crate::speech_sanitize::contains_tool_markup(&self.buf)
    }
}

fn sanitize_chunk(raw: &str) -> Option<String> {
    let cleaned = crate::speech_sanitize::strip_tool_markup(raw);
    let cleaned = cleaned.trim();
    (cleaned.chars().filter(|c| c.is_alphanumeric()).count() >= 2).then(|| cleaned.to_string())
}

const ABBREVIATIONS: &[&str] = &[
    "e.g", "i.e", "etc", "vs", "mr", "mrs", "ms", "dr", "st", "no", "approx", "jr", "sr",
];

/// Byte offset just past the next complete sentence in `buf[from..]`, or
/// `None` if the text so far does not end one yet.
fn next_boundary(buf: &str, from: usize) -> Option<usize> {
    let tail = &buf[from..];
    let mut chars = tail.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\n' {
            let end = from + i + 1;
            if has_words(&buf[from..end]) {
                return Some(end);
            }
            continue;
        }
        if !matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？') {
            continue;
        }
        // Closing quotes / brackets stay with the sentence.
        let mut end = i + c.len_utf8();
        while let Some(&(j, q)) = chars.peek() {
            if matches!(q, '"' | '\'' | '”' | '’' | ')' | ']') {
                end = j + q.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        // Need the following character to know the sentence really ended
        // ("3.5", "file.rs", "..." mid-thought).
        let Some(&(_, next)) = chars.peek() else {
            return None;
        };
        if !next.is_whitespace() {
            continue;
        }
        let sentence = &tail[..end];
        if c == '.' && is_abbreviation_or_list_marker(&tail[..i]) {
            continue;
        }
        if has_words(sentence) {
            return Some(from + end);
        }
    }
    None
}

fn has_words(s: &str) -> bool {
    s.chars().filter(|c| c.is_alphanumeric()).count() >= 2
}

/// `before` is the sentence text up to (not including) a period.
fn is_abbreviation_or_list_marker(before: &str) -> bool {
    let word = before
        .rsplit(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("");
    if word.is_empty() {
        return false;
    }
    let lower = word.to_ascii_lowercase();
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return true;
    }
    // A single initial ("J. Smith").
    if word.chars().count() == 1 && word.chars().all(char::is_alphabetic) {
        return true;
    }
    // "1." at the start of a numbered list item.
    word.chars().all(|c| c.is_ascii_digit()) && before.trim() == word
}

/// `full` without its leading `prefix`, comparing non-whitespace characters only.
fn strip_prefix_ignoring_ws<'a>(full: &'a str, prefix: &str) -> Option<&'a str> {
    let mut rest = full.char_indices();
    for want in prefix.chars().filter(|c| !c.is_whitespace()) {
        loop {
            match rest.next() {
                Some((_, c)) if c.is_whitespace() => continue,
                Some((_, c)) if c == want => break,
                _ => return None,
            }
        }
    }
    Some(rest.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(signals: &[SpeechSignal]) -> Vec<&str> {
        signals
            .iter()
            .filter_map(|s| match s {
                SpeechSignal::Chunk(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn released(seg: &mut SpeechSegmenter, text: &str) -> Vec<SpeechSignal> {
        let t0 = Instant::now();
        let mut out = seg.push_content(text, t0);
        out.extend(seg.poll(t0 + Duration::from_secs(5)));
        out
    }

    #[test]
    fn boundaries_skip_decimals_abbreviations_and_list_numbers() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        let out = released(
            &mut seg,
            "Version 3.5 is out, e.g. on crates.io today. Dr. Lee agrees! 1. Install it.\nThen ",
        );
        assert_eq!(
            chunks(&out),
            vec![
                "Version 3.5 is out, e.g. on crates.io today.",
                "Dr. Lee agrees!",
                "1. Install it.",
            ]
        );
    }

    #[test]
    fn sentence_without_following_char_waits() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        let t0 = Instant::now();
        assert!(seg.push_content("It is sunny.", t0).is_empty());
        let out = seg.push_content(" Highs of 20.", t0);
        assert_eq!(chunks(&out), vec!["It is sunny."]);
    }

    #[test]
    fn first_sentence_is_held_for_the_grace_period() {
        let mut seg = SpeechSegmenter::new(Duration::from_millis(900));
        let t0 = Instant::now();
        assert!(seg.push_content("It is sunny. More", t0).is_empty());
        assert!(seg.poll(t0 + Duration::from_millis(500)).is_empty());
        let out = seg.push_content(" text. Next", t0 + Duration::from_millis(950));
        assert_eq!(chunks(&out), vec!["It is sunny.", "More text."]);
        // After release, new sentences go straight out.
        let out = seg.push_content(" one. ", t0 + Duration::from_millis(960));
        assert_eq!(chunks(&out), vec!["Next one."]);
    }

    #[test]
    fn tool_call_before_release_is_silent() {
        let mut seg = SpeechSegmenter::new(Duration::from_millis(900));
        let t0 = Instant::now();
        assert!(seg.push_content("Let me check the logs. ", t0).is_empty());
        assert!(seg.on_tool_call().is_empty(), "nothing was spoken");
        assert!(seg.poll(t0 + Duration::from_secs(5)).is_empty());
        assert!(seg
            .push_content("More. ", t0 + Duration::from_secs(5))
            .is_empty());
    }

    #[test]
    fn late_tool_call_cancels_released_speech() {
        let mut seg = SpeechSegmenter::new(Duration::from_millis(100));
        let out = released(&mut seg, "Let me look into that for you. ");
        assert_eq!(chunks(&out), vec!["Let me look into that for you."]);
        assert_eq!(seg.on_tool_call(), vec![SpeechSignal::Cancel]);
        assert!(!seg.has_emitted());
        // Tools withheld at the cap: the same content becomes the answer and
        // is spoken from scratch.
        let out = seg.finish("Let me look into that for you.");
        assert_eq!(
            out,
            vec![
                SpeechSignal::Chunk("Let me look into that for you.".into()),
                SpeechSignal::Commit,
            ]
        );
    }

    #[test]
    fn finish_streams_the_unspoken_rest_then_commits() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        let out = released(&mut seg, "First part.  Second ");
        assert_eq!(chunks(&out), vec!["First part."]);
        let out = seg.finish("First part. Second part, the tail.");
        assert_eq!(
            out,
            vec![
                SpeechSignal::Chunk("Second part, the tail.".into()),
                SpeechSignal::Commit,
            ]
        );
    }

    #[test]
    fn finish_without_streaming_sends_the_whole_reply() {
        let mut seg = SpeechSegmenter::default();
        let t0 = Instant::now();
        assert!(seg.push_content("Short answer.", t0).is_empty());
        assert_eq!(
            seg.finish("Short answer."),
            vec![
                SpeechSignal::Chunk("Short answer.".into()),
                SpeechSignal::Commit
            ]
        );
    }

    #[test]
    fn diverging_final_text_cancels_instead_of_committing() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        released(&mut seg, "Streamed sentence here. ");
        assert_eq!(
            seg.finish("Something else entirely."),
            vec![SpeechSignal::Cancel]
        );
    }

    #[test]
    fn silence_and_markup_are_never_streamed() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        assert!(released(&mut seg, "[silent] ").is_empty());
        assert!(seg.finish("[silent]").is_empty());

        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        assert!(released(&mut seg, "<tool_call>{\"name\": \"x\"}</tool_call>. Done. ").is_empty());

        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        assert!(released(&mut seg, "Here:\n```\nfn main() {}\n").is_empty());
    }

    #[test]
    fn reset_cancels_streamed_chunks() {
        let mut seg = SpeechSegmenter::new(Duration::ZERO);
        released(&mut seg, "Spoken already. ");
        assert_eq!(seg.reset(), vec![SpeechSignal::Cancel]);
        assert!(seg.reset().is_empty());
    }
}
