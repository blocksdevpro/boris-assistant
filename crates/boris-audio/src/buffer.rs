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
    capacity: usize,
    is_recording: bool,
}

impl RecordingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
            is_recording: false,
        }
    }

    pub fn set_recording(&mut self, is_recording: bool) {
        self.is_recording = is_recording;
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
        } else {
            self.buffer.extend(value.iter().copied());
        }
    }

    /// Extracts the recorded audio and empties the buffer
    pub fn take_audio(&mut self) -> AudioBuffer {
        // Drain the entire buffer into a Vec
        self.buffer.drain(..).collect()
    }
}
