use super::*;
use boris_core::TurnId;

fn effects_kinds(effects: &[Effect]) -> Vec<&'static str> {
    effects
        .iter()
        .map(|e| match e {
            Effect::ArmWakeword => "ArmWakeword",
            Effect::DisarmWakeword => "DisarmWakeword",
            Effect::StartListen { .. } => "StartListen",
            Effect::StopListen => "StopListen",
            Effect::WarmStt => "WarmStt",
            Effect::Transcribe { .. } => "Transcribe",
            Effect::WarmTts => "WarmTts",
            Effect::Chat { .. } => "Chat",
            Effect::Synthesize { .. } => "Synthesize",
            Effect::Play { .. } => "Play",
        })
        .collect()
}

#[test]
fn wake_from_idle_starts_listen() {
    let mut s = Session::new();
    let effects = s.handle(SessionInput::WakeHit);
    assert!(matches!(s.state(), SessionState::Listening { turn } if turn.0 == 1));
    assert_eq!(
        effects_kinds(&effects),
        vec!["DisarmWakeword", "StartListen", "WarmStt"]
    );
}

#[test]
fn wake_while_busy_is_ignored() {
    let mut s = Session::new();
    s.handle(SessionInput::WakeHit);
    let effects = s.handle(SessionInput::WakeHit);
    assert!(effects.is_empty());
    assert!(matches!(s.state(), SessionState::Listening { .. }));
}

#[test]
fn happy_path_returns_to_idle() {
    let mut s = Session::new();
    s.handle(SessionInput::WakeHit);
    let turn = TurnId(1);

    s.handle(SessionInput::Endpoint);
    assert!(matches!(s.state(), SessionState::AwaitingClip { .. }));

    s.handle(SessionInput::ClipReady {
        turn,
        audio: vec![0.0; 160],
    });
    assert!(matches!(s.state(), SessionState::Transcribing { .. }));

    s.handle(SessionInput::Transcript {
        turn,
        text: "hello".into(),
    });
    assert!(matches!(s.state(), SessionState::Thinking { .. }));

    s.handle(SessionInput::AgentDone {
        turn,
        text: "yo bro".into(),
    });
    assert!(matches!(s.state(), SessionState::Speaking { .. }));

    let play = s.handle(SessionInput::TtsReady {
        turn,
        pcm: vec![0.1; 480],
    });
    assert_eq!(effects_kinds(&play), vec!["Play"]);
    assert!(matches!(s.state(), SessionState::Speaking { .. }));

    let done = s.handle(SessionInput::PlaybackFinished { turn });
    assert_eq!(effects_kinds(&done), vec!["ArmWakeword"]);
    assert!(s.state().is_idle());
}

#[test]
fn stale_transcript_is_dropped() {
    let mut s = Session::new();
    s.handle(SessionInput::WakeHit);
    s.handle(SessionInput::Endpoint);
    s.handle(SessionInput::ClipReady {
        turn: TurnId(1),
        audio: vec![0.0; 16],
    });

    let effects = s.handle(SessionInput::Transcript {
        turn: TurnId(99),
        text: "ghost".into(),
    });
    assert!(effects.is_empty());
    assert!(matches!(s.state(), SessionState::Transcribing { turn } if turn.0 == 1));
}

#[test]
fn service_failed_recovers_to_idle() {
    let mut s = Session::new();
    s.handle(SessionInput::WakeHit);
    let effects = s.handle(SessionInput::ServiceFailed {
        turn: Some(TurnId(1)),
        worker: "SttWorker",
        message: "boom".into(),
    });
    assert!(s.state().is_idle());
    assert!(effects_kinds(&effects).contains(&"ArmWakeword"));
    assert!(effects_kinds(&effects).contains(&"StopListen"));
}

#[test]
fn empty_agent_reply_recovers() {
    let mut s = Session::new();
    s.handle(SessionInput::WakeHit);
    let turn = TurnId(1);
    s.handle(SessionInput::Endpoint);
    s.handle(SessionInput::ClipReady {
        turn,
        audio: vec![0.0; 8],
    });
    s.handle(SessionInput::Transcript {
        turn,
        text: "hi".into(),
    });
    let effects = s.handle(SessionInput::AgentDone {
        turn,
        text: "   ".into(),
    });
    assert!(s.state().is_idle());
    assert_eq!(effects_kinds(&effects), vec!["ArmWakeword"]);
}
