//! Ring / recording buffers for wake + utterance capture.
//!
//! - [`SlidingBuffer`] — fixed-size rolling window (wake-word scoring).
//! - [`RecordingBuffer`] — pre-roll while idle, then grow until endpoint / max.

use std::collections::VecDeque;

use boris_core::{AudioBuffer, AudioSample};

// ── shared sliding-window helper ─────────────────────────────────────────────

/// Keep the newest `capacity` samples in `buf`, then append `chunk`.
fn push_keep_newest(buf: &mut VecDeque<AudioSample>, capacity: usize, chunk: &[AudioSample]) {
    if chunk.is_empty() {
        return;
    }
    if capacity == 0 {
        buf.clear();
        return;
    }

    let overflow = buf
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(capacity);
    if overflow > 0 {
        buf.drain(..overflow.min(buf.len()));
    }
    buf.extend(chunk.iter().copied());

    // Incoming chunk larger than capacity: keep the newest `capacity` samples.
    if buf.len() > capacity {
        let excess = buf.len() - capacity;
        buf.drain(..excess);
    }
}

// ── SlidingBuffer ────────────────────────────────────────────────────────────

/// Fixed-capacity rolling window of mono PCM (newest samples win).
///
/// Used by wake-word scoring: once [`SlidingBuffer::ready`], [`SlidingBuffer::read`]
/// returns exactly `capacity` samples (or fewer only before the window fills).
pub struct SlidingBuffer {
    buffer: VecDeque<AudioSample>,
    capacity: usize,
}

impl SlidingBuffer {
    /// Create a buffer that retains at most `capacity` samples.
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Maximum samples retained.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of samples held.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Append samples, dropping the oldest when over capacity.
    pub fn push(&mut self, value: &[AudioSample]) {
        push_keep_newest(&mut self.buffer, self.capacity, value);
    }

    /// True once the window is full (`len == capacity`).
    pub fn ready(&self) -> bool {
        self.capacity > 0 && self.buffer.len() >= self.capacity
    }

    /// Copy the newest `min(capacity, len)` samples into a new [`AudioBuffer`].
    pub fn read(&self) -> AudioBuffer {
        let size = self.capacity.min(self.buffer.len());
        let start = self.buffer.len() - size;
        self.buffer.range(start..).copied().collect()
    }

    /// Drop all samples.
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

// ── RecordingBuffer ──────────────────────────────────────────────────────────

/// Utterance buffer with idle pre-roll and a hard recording length cap.
///
/// While **not** recording, behaves like a sliding window of `capacity` samples
/// (pre-roll so STT hears the start of speech). While recording, grows until
/// [`RecordingBuffer::take_audio`] or [`RecordingBuffer::exceeded_max`].
pub struct RecordingBuffer {
    buffer: VecDeque<AudioSample>,
    /// Sliding pre-roll size while idle (not recording).
    capacity: usize,
    /// Hard cap on samples while actively recording (utterance length limit).
    max_recording_samples: usize,
    is_recording: bool,
    /// Set once active recording hits [`Self::max_recording_samples`].
    exceeded_max: bool,
}

impl RecordingBuffer {
    /// `capacity` is the pre-roll window (samples).
    /// `max_recording_samples` is the hard cap while recording (e.g. 30s @ 16 kHz).
    pub fn new(capacity: usize, max_recording_samples: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            max_recording_samples: max_recording_samples.max(1),
            is_recording: false,
            exceeded_max: false,
        }
    }

    /// Pre-roll window size (samples).
    pub fn preroll_capacity(&self) -> usize {
        self.capacity
    }

    /// Hard cap while recording (samples).
    pub fn max_recording_samples(&self) -> usize {
        self.max_recording_samples
    }

    /// Current sample count.
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Whether capture is in the active recording phase.
    pub fn is_recording(&self) -> bool {
        self.is_recording
    }

    /// Enter / leave active recording.
    ///
    /// - Entering (`true`) clears the exceeded flag and keeps the current buffer
    ///   (pre-roll is the start of the utterance).
    /// - Leaving (`false`) only flips the flag. It does **not** trim samples —
    ///   callers must [`Self::take_audio`] the full utterance first. Subsequent
    ///   idle [`Self::push`] calls already enforce the pre-roll window via
    ///   sliding retention, so a full clip is never silently truncated on stop.
    pub fn set_recording(&mut self, is_recording: bool) {
        if is_recording {
            self.is_recording = true;
            self.exceeded_max = false;
            return;
        }
        self.is_recording = false;
    }

    /// True after a recording push hit the utterance length cap.
    pub fn exceeded_max(&self) -> bool {
        self.exceeded_max
    }

    /// Append samples (pre-roll or recording, depending on state).
    pub fn push(&mut self, value: &[AudioSample]) {
        if value.is_empty() {
            return;
        }
        if !self.is_recording {
            push_keep_newest(&mut self.buffer, self.capacity, value);
            return;
        }
        if self.exceeded_max {
            return;
        }

        let room = self.max_recording_samples.saturating_sub(self.buffer.len());
        if room == 0 {
            self.exceeded_max = true;
            return;
        }
        if value.len() <= room {
            self.buffer.extend(value.iter().copied());
        } else {
            self.buffer.extend(value.iter().copied().take(room));
            self.exceeded_max = true;
        }
        if self.buffer.len() >= self.max_recording_samples {
            self.exceeded_max = true;
        }
    }

    /// Take all samples and empty the buffer (also clears `exceeded_max`).
    pub fn take_audio(&mut self) -> AudioBuffer {
        self.exceeded_max = false;
        self.buffer.drain(..).collect()
    }

    /// Drop samples without returning them.
    pub fn clear(&mut self) {
        self.buffer.clear();
        self.exceeded_max = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_keeps_newest() {
        let mut b = SlidingBuffer::new(4);
        assert!(!b.ready());
        b.push(&[1.0, 2.0, 3.0]);
        assert!(!b.ready());
        b.push(&[4.0, 5.0]);
        assert!(b.ready());
        assert_eq!(b.read(), vec![2.0, 3.0, 4.0, 5.0]);
        b.push(&[9.0]);
        assert_eq!(b.read(), vec![3.0, 4.0, 5.0, 9.0]);
    }

    #[test]
    fn sliding_chunk_larger_than_capacity() {
        let mut b = SlidingBuffer::new(3);
        b.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(b.read(), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn recording_preroll_then_grow() {
        let mut r = RecordingBuffer::new(3, 10);
        r.push(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(r.len(), 3); // preroll keeps newest 3

        r.set_recording(true);
        r.push(&[5.0, 6.0]);
        assert_eq!(r.len(), 5);
        assert!(!r.exceeded_max());

        let audio = r.take_audio();
        assert_eq!(audio, vec![2.0, 3.0, 4.0, 5.0, 6.0]);
        assert!(r.is_empty());
    }

    #[test]
    fn recording_hits_max() {
        let mut r = RecordingBuffer::new(2, 5);
        r.set_recording(true);
        r.push(&[1.0, 2.0, 3.0]);
        r.push(&[4.0, 5.0, 6.0, 7.0]);
        assert!(r.exceeded_max());
        assert_eq!(r.len(), 5);
        assert_eq!(r.take_audio(), vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn set_recording_false_keeps_full_utterance_for_take() {
        // Regression: stopping used to trim to pre-roll *before* take_audio,
        // so STT only ever saw the last ~preroll seconds of speech.
        let mut r = RecordingBuffer::new(3, 20);
        r.push(&[1.0, 2.0, 3.0]);
        r.set_recording(true);
        r.push(&[4.0, 5.0, 6.0, 7.0]);
        assert_eq!(r.len(), 7);

        r.set_recording(false);
        assert!(!r.is_recording());
        // Full clip still present until drained.
        assert_eq!(r.len(), 7);
        assert_eq!(r.take_audio(), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn capture_endpoint_order_preserves_long_utterance() {
        // Mirrors hear::capture_utterance: take_audio then set_recording(false).
        let mut r = RecordingBuffer::new(3, 20);
        r.set_recording(true);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        let clip = r.take_audio();
        r.set_recording(false);
        assert_eq!(clip, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
        assert!(r.is_empty());
    }

    #[test]
    fn idle_push_after_stop_enforces_preroll() {
        let mut r = RecordingBuffer::new(3, 20);
        r.set_recording(true);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        r.set_recording(false);
        // Next idle push slides to pre-roll capacity.
        r.push(&[6.0]);
        assert_eq!(r.take_audio(), vec![4.0, 5.0, 6.0]);
    }
}
