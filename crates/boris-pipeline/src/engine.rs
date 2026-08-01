//! Single-threaded sequential voice engine for desktop.
//!
//! One background thread owns audio + models and walks a turn top-to-bottom.
//! Phase updates are for the UI only — control flow is ordinary `?` / `match`.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use boris_agent::{AgentEngine, AgentOutcome, OpenRouterClient};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::TurnId;
use boris_inference::{SpeechToText, TextToSpeech};
use boris_sense::{init_onnx_runtime, LivekitWakeWord, WebRtcVad};

use crate::config::PipelineConfig;
use crate::devices::{find_input, find_output};
use crate::hear::{self, HearBreak};
use crate::status::{DeviceHealth, EngineState, Phase, StatusPicture};

const MIC_QUEUE: usize = 64;

#[derive(Debug)]
pub enum EngineCommand {
    Start,
    Stop,
    Shutdown,
    SwitchInput { device_id: String },
    SwitchOutput { device_id: String },
}

#[derive(Clone)]
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
}

impl EngineHandle {
    pub fn send(&self, cmd: EngineCommand) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.cmd_tx.send(cmd)
    }

    pub fn start(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Start)
    }

    pub fn stop(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Stop)
    }

    pub fn shutdown(&self) -> Result<(), mpsc::SendError<EngineCommand>> {
        self.send(EngineCommand::Shutdown)
    }
}

/// Join handle for the engine thread (drop does not join).
pub struct Engine {
    _join: JoinHandle<()>,
}

impl Engine {
    /// Spawn the engine thread. Status snapshots are sent on the returned receiver.
    pub fn spawn(config: PipelineConfig) -> (Self, EngineHandle, Receiver<StatusPicture>) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let join = thread::Builder::new()
            .name("boris-engine".into())
            .spawn(move || {
                if let Err(e) = run(config, cmd_rx, status_tx) {
                    tracing::error!(error = %e, "engine thread exited with error");
                }
            })
            .expect("spawn boris-engine");

        (Self { _join: join }, EngineHandle { cmd_tx }, status_rx)
    }
}

struct Picture {
    engine: EngineState,
    phase: Phase,
    detail: Option<String>,
    heard: Option<String>,
    said: Option<String>,
    mic: DeviceHealth,
    speaker: DeviceHealth,
    turn: Option<TurnId>,
    status_tx: Sender<StatusPicture>,
}

impl Picture {
    fn publish(&self) {
        let _ = self.status_tx.send(StatusPicture {
            engine: self.engine,
            phase: self.phase,
            detail: self.detail.clone(),
            heard: self.heard.clone(),
            said: self.said.clone(),
            mic: self.mic.clone(),
            speaker: self.speaker.clone(),
            turn: self.turn.map(|t| t.to_string()),
        });
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.publish();
    }
}

fn run(
    config: PipelineConfig,
    cmd_rx: Receiver<EngineCommand>,
    status_tx: Sender<StatusPicture>,
) -> Result<(), String> {
    init_onnx_runtime();

    let mut audio = match AudioService::with_source_rate(config.play_source_rate) {
        Ok(audio) => audio,
        Err(e) => {
            let _ = status_tx.send(StatusPicture {
                engine: EngineState::Fault,
                phase: Phase::Off,
                detail: Some(e.clone()),
                heard: None,
                said: None,
                mic: DeviceHealth {
                    label: config.mic_label.clone(),
                    ok: false,
                },
                speaker: DeviceHealth {
                    label: config.speaker_label.clone(),
                    ok: false,
                },
                turn: None,
            });
            return Err(format!("audio init failed: {e}"));
        }
    };
    let mic = audio.subscribe_input(Some(MIC_QUEUE));
    let mut output_events = audio.subscribe_output();

    let mut wake = LivekitWakeWord::new(
        "boris",
        &config.wakeword_model,
        boris_audio::AUDIO_TARGET_RATE,
    );
    let mut vad = WebRtcVad::new();

    tracing::info!(
        stt = %config.stt_model_dir.display(),
        tts = %config.tts_model_dir.display(),
        voices = %config.tts_voice_dir.display(),
        "model paths"
    );

    #[cfg(feature = "stt-parakeet")]
    let mut stt: Box<dyn SpeechToText> = Box::new(boris_stt_parakeet::ParakeetStt::with_model_dir(
        config.stt_model_dir.clone(),
    ));
    #[cfg(not(feature = "stt-parakeet"))]
    let mut stt: Box<dyn SpeechToText> = Box::new(NullStt);

    #[cfg(feature = "tts-supertone")]
    let mut tts: Box<dyn TextToSpeech> = Box::new(boris_tts_supertone::SupertoneTts::with_paths(
        config.tts_model_dir.clone(),
        config.tts_voice_dir.clone(),
        &config.tts_voice_id,
    ));
    #[cfg(not(feature = "tts-supertone"))]
    let mut tts: Box<dyn TextToSpeech> = Box::new(NullTts);

    let client = OpenRouterClient::new(config.openrouter_api_key, config.openrouter_model);
    let mut agent = AgentEngine::new(Box::new(client), &config.system_prompt);

    let mut picture = Picture {
        engine: EngineState::Off,
        phase: Phase::Off,
        detail: None,
        heard: None,
        said: None,
        mic: DeviceHealth {
            label: config.mic_label,
            ok: true,
        },
        speaker: DeviceHealth {
            label: config.speaker_label,
            ok: true,
        },
        turn: None,
        status_tx,
    };
    picture.publish();

    let mut running = false;
    let mut next_turn: u64 = 1;

    loop {
        // ── Off: wait for Start ─────────────────────────────────────────────
        if !running {
            match cmd_rx.recv() {
                Ok(EngineCommand::Start) => {
                    running = true;
                    picture.engine = EngineState::On;
                    picture.detail = None;
                    picture.heard = None;
                    picture.said = None;
                    picture.turn = None;
                    picture.set_phase(Phase::Armed);
                    tracing::info!("engine started");
                }
                Ok(EngineCommand::Stop) => continue,
                Ok(EngineCommand::Shutdown) | Err(_) => {
                    picture.engine = EngineState::Off;
                    picture.set_phase(Phase::Off);
                    return Ok(());
                }
                Ok(EngineCommand::SwitchInput { device_id }) => {
                    apply_input_switch(&mut audio, &mut picture, &device_id);
                }
                Ok(EngineCommand::SwitchOutput { device_id }) => {
                    apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                }
            }
            continue;
        }

        // ── Armed: wait for wake (interruptible) ────────────────────────────
        picture.heard = None;
        picture.said = None;
        picture.detail = None;
        picture.turn = None;
        picture.set_phase(Phase::Armed);

        match hear::wait_for_wake(&mic, &mut wake, &cmd_rx, &mut running) {
            Ok(()) => {}
            Err(HearBreak::SwitchInput { device_id }) => {
                apply_input_switch(&mut audio, &mut picture, &device_id);
                continue; // re-arm with new mic
            }
            Err(HearBreak::SwitchOutput { device_id }) => {
                apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                continue;
            }
            Err(HearBreak::Stopped) if !running => {
                audio.stop();
                picture.engine = EngineState::Off;
                picture.set_phase(Phase::Off);
                continue;
            }
            Err(HearBreak::Stopped) => continue,
            Err(HearBreak::Disconnected) => {
                picture.set_phase(Phase::Off);
                return Ok(());
            }
        }

        if !running {
            picture.engine = EngineState::Off;
            picture.set_phase(Phase::Off);
            continue;
        }

        // ── One turn, top to bottom ─────────────────────────────────────────
        let turn = TurnId(next_turn);
        next_turn = next_turn.saturating_add(1);
        picture.turn = Some(turn);
        picture.set_phase(Phase::Hearing);
        tracing::info!(%turn, "turn begin — hearing");

        let clip = match hear::capture_utterance(&mic, &mut vad, &cmd_rx, &mut running) {
            Ok(c) => c,
            Err(HearBreak::SwitchInput { device_id }) => {
                apply_input_switch(&mut audio, &mut picture, &device_id);
                continue; // drop partial clip; re-arm
            }
            Err(HearBreak::SwitchOutput { device_id }) => {
                apply_output_switch(&mut audio, &mut output_events, &mut picture, &device_id);
                continue;
            }
            Err(HearBreak::Stopped) if !running => {
                audio.stop();
                picture.engine = EngineState::Off;
                picture.set_phase(Phase::Off);
                continue;
            }
            Err(HearBreak::Stopped) => continue,
            Err(HearBreak::Disconnected) => return Ok(()),
        };

        if !running {
            picture.engine = EngineState::Off;
            picture.set_phase(Phase::Off);
            continue;
        }

        // Read
        picture.set_phase(Phase::Reading);
        if let Err(e) = stt.load() {
            tracing::error!(error = %e, %turn, "stt load failed");
            picture.detail = Some(format!("stt load: {e}"));
            picture.set_phase(Phase::Armed);
            continue;
        }
        let text = match stt.transcribe(&clip) {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(error = %e, %turn, "stt failed");
                picture.detail = Some(format!("stt: {e}"));
                let _ = stt.unload();
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        let _ = stt.unload();
        picture.heard = Some(text.clone());
        picture.publish();
        tracing::info!(%turn, %text, "heard");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        ) {
            go_off(&mut picture, &audio);
            continue;
        }

        // Think
        picture.set_phase(Phase::Thinking);
        let reply = match agent.chat(&text) {
            Ok(AgentOutcome::Speak(s)) if !s.trim().is_empty() => s,
            Ok(_) => {
                tracing::warn!(%turn, "agent produced no speech");
                picture.detail = Some("empty agent reply".into());
                picture.set_phase(Phase::Armed);
                continue;
            }
            Err(e) => {
                tracing::error!(error = %e, %turn, "agent failed");
                picture.detail = Some(format!("agent: {e}"));
                picture.set_phase(Phase::Armed);
                continue;
            }
        };
        picture.said = Some(reply.clone());
        picture.publish();
        tracing::info!(%turn, %reply, "said");

        if !poll_running(
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut output_events,
            &mut picture,
        ) {
            go_off(&mut picture, &audio);
            continue;
        }

        // Talk
        picture.set_phase(Phase::Talking);
        if let Err(e) = tts.load() {
            tracing::error!(error = %e, %turn, "tts load failed");
            picture.detail = Some(format!("tts load: {e}"));
            picture.set_phase(Phase::Armed);
            continue;
        }
        let pcm = match tts.synthesize(&reply) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, %turn, "tts failed");
                picture.detail = Some(format!("tts: {e}"));
                picture.set_phase(Phase::Armed);
                continue;
            }
        };

        // Drain any stale output events, then play and wait for drain.
        while output_events.try_recv().is_ok() {}
        audio.play(pcm);
        wait_playback_or_stop(
            &mut output_events,
            &cmd_rx,
            &mut running,
            &mut audio,
            &mut picture,
        );

        if !running {
            go_off(&mut picture, &audio);
            continue;
        }

        tracing::info!(%turn, "turn complete");
        // loop → Armed again
    }
}

fn go_off(picture: &mut Picture, audio: &AudioService) {
    audio.stop();
    picture.engine = EngineState::Off;
    picture.turn = None;
    picture.set_phase(Phase::Off);
}

/// Drain host commands; apply device switches immediately. Returns false if stopped.
fn poll_running(
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
) -> bool {
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Stop) | Ok(EngineCommand::Shutdown) => {
                *running = false;
                return false;
            }
            Ok(EngineCommand::Start) => *running = true,
            Ok(EngineCommand::SwitchInput { device_id }) => {
                apply_input_switch(audio, picture, &device_id);
            }
            Ok(EngineCommand::SwitchOutput { device_id }) => {
                apply_output_switch(audio, output_events, picture, &device_id);
            }
            Err(mpsc::TryRecvError::Empty) => return *running,
            Err(mpsc::TryRecvError::Disconnected) => {
                *running = false;
                return false;
            }
        }
    }
}

fn wait_playback_or_stop(
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    cmd_rx: &Receiver<EngineCommand>,
    running: &mut bool,
    audio: &mut AudioService,
    picture: &mut Picture,
) {
    loop {
        if !poll_running(cmd_rx, running, audio, output_events, picture) {
            audio.stop();
            return;
        }
        match output_events.recv_timeout(std::time::Duration::from_millis(40)) {
            Ok(OutputEvent::Drained) => return,
            Ok(OutputEvent::Cleared) => return,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn apply_input_switch(audio: &mut AudioService, picture: &mut Picture, device_id: &str) {
    match find_input(device_id) {
        Some(info) => match audio.switch_input(&info.id) {
            Ok(()) => {
                tracing::info!(name = %info.name, "switched input device");
                picture.mic = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "input switch failed");
                picture.detail = Some(format!("mic switch failed: {e}"));
                picture.mic.ok = false;
                picture.publish();
            }
        },
        None => {
            tracing::warn!(%device_id, "unknown input device");
            picture.detail = Some(format!("unknown microphone id"));
            picture.publish();
        }
    }
}

fn apply_output_switch(
    audio: &mut AudioService,
    output_events: &mut crossbeam_channel::Receiver<OutputEvent>,
    picture: &mut Picture,
    device_id: &str,
) {
    match find_output(device_id) {
        Some(info) => match audio.switch_output(&info.id) {
            Ok(()) => {
                tracing::info!(name = %info.name, "switched output device");
                // Output pipeline rebuilds its event channel — resubscribe.
                *output_events = audio.subscribe_output();
                picture.speaker = DeviceHealth {
                    label: info.name,
                    ok: true,
                };
                picture.detail = None;
                picture.publish();
            }
            Err(e) => {
                tracing::error!(error = %e, %device_id, "output switch failed");
                picture.detail = Some(format!("speaker switch failed: {e}"));
                picture.speaker.ok = false;
                picture.publish();
            }
        },
        None => {
            tracing::warn!(%device_id, "unknown output device");
            picture.detail = Some(format!("unknown speaker id"));
            picture.publish();
        }
    }
}

// ── Optional null backends when features are off ─────────────────────────────

#[cfg(not(feature = "stt-parakeet"))]
struct NullStt;

#[cfg(not(feature = "stt-parakeet"))]
impl SpeechToText for NullStt {
    fn transcribe(&mut self, _: &[boris_core::AudioSample]) -> boris_core::error::Result<String> {
        Err(boris_core::error::Error::Other(
            "stt-parakeet feature disabled".into(),
        ))
    }
}

#[cfg(not(feature = "tts-supertone"))]
struct NullTts;

#[cfg(not(feature = "tts-supertone"))]
impl TextToSpeech for NullTts {
    fn synthesize(&mut self, _: &str) -> boris_core::error::Result<boris_core::AudioBuffer> {
        Err(boris_core::error::Error::Other(
            "tts-supertone feature disabled".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
