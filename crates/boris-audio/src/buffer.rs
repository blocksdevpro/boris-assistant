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
        for &sample in value {
            if self.buffer.len() == self.capacity {
                self.buffer.pop_front(); // O(1), no shifting
            }
            self.buffer.push_back(sample); // O(1) amortized
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
        for &sample in value {
            // If we are NOT recording and the buffer is full, pop the oldest sample
            // to maintain our pre-roll window (e.g. 2 seconds of audio)
            if !self.is_recording && self.buffer.len() == self.capacity {
                self.buffer.pop_front();
            }
            self.buffer.push_back(sample);
        }
    }

    /// Extracts the recorded audio and empties the buffer
    pub fn take_audio(&mut self) -> AudioBuffer {
        // Drain the entire buffer into a Vec
        self.buffer.drain(..).collect()
    }
}
