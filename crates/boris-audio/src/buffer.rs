use std::collections::VecDeque;

use boris_core::{AudioBuffer, AudioSample};

pub struct SlidingBuffer {
    buffer: VecDeque<AudioSample>,
    capacity: usize,
}

impl SlidingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, value: &[AudioSample]) {
        if value.is_empty() {
            return;
        }
        // Bulk-drop oldest samples when the sliding window would overflow.
        let overflow = self
            .buffer
            .len()
            .saturating_add(value.len())
            .saturating_sub(self.capacity);
        if overflow > 0 {
            self.buffer.drain(..overflow.min(self.buffer.len()));
        }
        self.buffer.extend(value.iter().copied());
        // Incoming chunk larger than capacity: keep the newest `capacity` samples.
        if self.buffer.len() > self.capacity {
            let excess = self.buffer.len() - self.capacity;
            self.buffer.drain(..excess);
        }
    }

    pub fn ready(&self) -> bool {
        self.buffer.len() >= self.capacity
    }

    pub fn read(&self) -> AudioBuffer {
        let size = self.capacity.min(self.buffer.len());
        let start = self.buffer.len() - size;
        self.buffer.range(start..).copied().collect()
    }
}

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

    pub fn set_recording(&mut self, is_recording: bool) {
        self.is_recording = is_recording;
        if is_recording {
            self.exceeded_max = false;
        }
    }

    /// True after a recording push hit the utterance length cap.
    pub fn exceeded_max(&self) -> bool {
        self.exceeded_max
    }

    pub fn push(&mut self, value: &[AudioSample]) {
        if value.is_empty() {
            return;
        }
        if !self.is_recording {
            // Pre-roll: keep only the last `capacity` samples.
            let overflow = self
                .buffer
                .len()
                .saturating_add(value.len())
                .saturating_sub(self.capacity);
            if overflow > 0 {
                self.buffer.drain(..overflow.min(self.buffer.len()));
            }
            self.buffer.extend(value.iter().copied());
            if self.buffer.len() > self.capacity {
                let excess = self.buffer.len() - self.capacity;
                self.buffer.drain(..excess);
            }
        } else if !self.exceeded_max {
            let room = self
                .max_recording_samples
                .saturating_sub(self.buffer.len());
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
    }

    /// Extracts the recorded audio and empties the buffer.
    pub fn take_audio(&mut self) -> AudioBuffer {
        self.exceeded_max = false;
        // Drain the entire buffer into a Vec
        self.buffer.drain(..).collect()
    }
}
