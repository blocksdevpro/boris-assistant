/// What the agent decided after one user message (tool loop finished).
///
/// The binary maps this into runtime events; `boris-agent` never emits speech
/// or touches the app event bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// Final plain-text reply — Session should synthesize and play this.
    Speak(String),
    /// Model returned no speakable content.
    Silent,
}
