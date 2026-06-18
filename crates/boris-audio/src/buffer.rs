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
