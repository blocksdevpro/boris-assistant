//! Shared speech capability traits (STT / TTS).
//!
//! Wake-word and VAD live in [`boris_sense`]. This crate keeps the model
//! service ports so adapter crates (`boris-stt-*`, `boris-tts-*`) do not
//! depend on perception.

use boris_core::{error::Result, AudioBuffer, AudioSample};

pub trait SpeechToText: Send {
    fn load(&mut self) -> Result<()> {
        Ok(())
    }
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }
    fn transcribe(&mut self, audio: &[AudioSample]) -> Result<String>;
}

pub trait TextToSpeech: Send {
    fn load(&mut self) -> Result<()> {
        Ok(())
    }
    fn unload(&mut self) -> Result<()> {
        Ok(())
    }
    fn synthesize(&mut self, text: &str) -> Result<AudioBuffer>;
}
