use boris_audio::service::AudioService;

pub type CrossbeamChannel<T> = (crossbeam_channel::Sender<T>, crossbeam_channel::Receiver<T>);

pub enum OrcCommand {
    Start,
    Stop,
    SwitchInput { device_id: String },
    SwitchOutput { device_id: String },
}

pub struct Orchestrator {
    channel: CrossbeamChannel<OrcCommand>,
    audio_service: AudioService,
}
