//! One-time engine thread setup: audio, wake, STT/TTS shells, agent + tools.

use boris_agent::session::store::SessionStore;
use boris_agent::{Agent, SandboxConfig};
use boris_audio::output::OutputEvent;
use boris_audio::service::AudioService;
use boris_core::ArcAudioBuffer;
use boris_sense::{init_onnx_runtime, LivekitWakeWord, WebRtcVad};

use crate::config::PipelineConfig;
use crate::paths;
use crate::status::{DeviceHealth, EngineState, Phase, StatusPicture, DEFAULT_CONTEXT_LIMIT_TOKENS};

use super::llm::{
    build_openrouter_client, looks_like_non_agent_model, resolve_model_and_provider,
    DEFAULT_STRONG_MODEL,
};
use super::models::{create_stt, create_tts, SttBox, TtsBox};
use super::picture::Picture;
use super::util::{env_flag_false, env_flag_true, panic_payload_str, uuid_lite};
use super::MIC_QUEUE;

/// Fully initialized engine-thread state before the turn loop.
pub(super) struct EngineRuntime {
    pub audio: AudioService,
    pub mic: crossbeam_channel::Receiver<ArcAudioBuffer>,
    pub output_events: crossbeam_channel::Receiver<OutputEvent>,
    pub wake: LivekitWakeWord,
    pub vad: WebRtcVad,
    pub stt: SttBox,
    pub tts: TtsBox,
    pub agent_rt: tokio::runtime::Runtime,
    pub agent: Agent,
    pub store: SessionStore,
    pub picture: Picture,
    /// Paths needed for model-load failure diagnostics during the loop.
    pub stt_model_dir: std::path::PathBuf,
    pub tts_model_dir: std::path::PathBuf,
    pub system_prompt: String,
}

/// Build audio + wake + models + agent. On hard init failure, publishes Fault and returns Err.
pub(super) fn init_runtime(
    config: PipelineConfig,
    status_tx: std::sync::mpsc::Sender<StatusPicture>,
) -> Result<EngineRuntime, String> {
    tracing::info!("engine thread entered run()");
    crate::diagnostics::log_environment("engine_run");
    crate::diagnostics::log_writable_check("boris_home", paths::boris_home());
    crate::diagnostics::log_writable_check("sessions", paths::sessions_dir());
    crate::diagnostics::log_writable_check("logs", paths::logs_dir());

    tracing::info!("init_onnx_runtime…");
    init_onnx_runtime();
    tracing::info!("init_onnx_runtime done");

    tracing::info!(
        play_source_rate = config.play_source_rate,
        wake_bytes = config.wakeword_model.len(),
        stt = %config.stt_model_dir.display(),
        tts = %config.tts_model_dir.display(),
        voices = %config.tts_voice_dir.display(),
        voice_id = %config.tts_voice_id,
        openrouter_model = ?config.openrouter_model,
        has_api_key = !config.openrouter_api_key.trim().is_empty(),
        "pipeline config (key redacted)"
    );

    let mut audio = open_audio(&config, &status_tx)?;
    let mic = audio.subscribe_input(Some(MIC_QUEUE));
    let output_events = audio.subscribe_output();
    tracing::info!(mic_queue = MIC_QUEUE, "subscribed to mic + output events");

    let wake = load_wakeword(&config, &status_tx)?;
    let vad = WebRtcVad::new();
    tracing::info!("WebRtcVad ready");

    let stt = create_stt(config.stt_model_dir.clone());
    let tts = create_tts(
        config.tts_model_dir.clone(),
        config.tts_voice_dir.clone(),
        &config.tts_voice_id,
    );

    // Long-lived Tokio runtime for the async agent plane (LLM + tools).
    // Voice capture / STT / TTS stay on this sync engine thread.
    let agent_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("boris-agent")
        .build()
        .expect("failed to build Tokio runtime for agent");

    let agent = build_agent(&config);

    // Session persistence under ~/.boris/sessions/desktop (soft-fail on I/O).
    if let Err(e) = paths::ensure_sessions_dir() {
        tracing::warn!(error = %e, "ensure sessions dir failed");
    }
    let store = SessionStore::new(paths::sessions_dir());

    let picture = Picture {
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
        activity: None,
        context_used: None,
        context_limit: Some(DEFAULT_CONTEXT_LIMIT_TOKENS),
        status_tx,
    };
    picture.publish();
    tracing::info!("engine idle (Off) — waiting for Start command");
Ok(EngineRuntime {
        audio,
        mic,
        output_events,
        wake,
        vad,
        stt,
        tts,
        agent_rt,
        agent,
        store,
        picture,
        stt_model_dir: config.stt_model_dir,
        tts_model_dir: config.tts_model_dir,
        system_prompt: config.system_prompt,
    })
}

fn open_audio(
    config: &PipelineConfig,
    status_tx: &std::sync::mpsc::Sender<StatusPicture>,
) -> Result<AudioService, String> {
    tracing::info!("opening AudioService (default mic + speaker)…");
    match AudioService::with_source_rate(config.play_source_rate) {
        Ok(audio) => {
            tracing::info!("AudioService ready");
            Ok(audio)
        }
        Err(e) => {
            tracing::error!(error = %e, "AudioService::with_source_rate FAILED");
            crate::diagnostics::log_environment("audio_init_failed");
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
                activity: None,
                context_used: None,
                context_limit: None,
            });
            Err(format!("audio init failed: {e}"))
        }
    }
}

fn load_wakeword(
    config: &PipelineConfig,
    status_tx: &std::sync::mpsc::Sender<StatusPicture>,
) -> Result<LivekitWakeWord, String> {
    tracing::info!(
        wake_bytes = config.wakeword_model.len(),
        sample_rate = boris_audio::AUDIO_TARGET_RATE,
        "loading LivekitWakeWord (ORT sessions)…"
    );
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        LivekitWakeWord::new(
            "boris",
            &config.wakeword_model,
            boris_audio::AUDIO_TARGET_RATE,
        )
    })) {
        Ok(w) => {
            tracing::info!("LivekitWakeWord loaded");
            Ok(w)
        }
        Err(payload) => {
            let msg = panic_payload_str(&payload);
            tracing::error!(
                error = %msg,
                wake_bytes = config.wakeword_model.len(),
                "LivekitWakeWord::new PANICKED — often missing onnxruntime.dll / DirectML.dll beside the exe"
            );
            crate::diagnostics::log_environment("wakeword_panic");
            let detail = format!("wakeword init panic: {msg}");
            let _ = status_tx.send(StatusPicture {
                engine: EngineState::Fault,
                phase: Phase::Off,
                detail: Some(detail.clone()),
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
                activity: None,
                context_used: None,
                context_limit: None,
            });
            Err(detail)
        }
    }
}

fn build_agent(config: &PipelineConfig) -> Agent {
    tracing::info!("building OpenRouter client + Agent…");
    // P1 model routing: fast for simple facts, strong for multi-step work.
    // Supports OpenRouter **model-provider** prefs (CoreWeave, Baseten, …) and
    // session sticky routing so prompt-cache hits show up as cached_tokens.
    let (strong_model, strong_provider_raw) = resolve_model_and_provider(
        config.openrouter_model.as_deref(),
        config.openrouter_model_provider.as_deref(),
        DEFAULT_STRONG_MODEL,
    );
    let (fast_model, fast_provider_raw) = resolve_model_and_provider(
        config
            .openrouter_fast_model
            .as_deref()
            .or(config.openrouter_model.as_deref()),
        config.openrouter_fast_provider.as_deref(),
        &strong_model,
    );
    let pin = config.openrouter_pin_provider;
    // Morph apply / merge models cannot do tool calling — Boris always sends tools.
    if looks_like_non_agent_model(&strong_model) {
        tracing::warn!(
            model = %strong_model,
            "configured OpenRouter model looks specialized (e.g. Morph apply) and usually \
             cannot use tools (get_time, bash, …). Prefer a chat model such as \
             google/gemini-2.5-flash-lite. Routing will fall back when tool use is rejected."
        );
    }
    // One session id for the engine process → OpenRouter sticky-routes to the
    // same host, improving cache hit rates on multi-turn agent work.
    let session_id = format!("boris-{}", uuid_lite());
    let strong = build_openrouter_client(
        &config.openrouter_api_key,
        &strong_model,
        strong_provider_raw.as_deref(),
        pin,
        &session_id,
    );
    let fast = build_openrouter_client(
        &config.openrouter_api_key,
        &fast_model,
        fast_provider_raw.as_deref(),
        pin,
        &session_id,
    );
    let client: Box<dyn boris_agent::LlmClient> = if fast_model == strong_model
        && strong_provider_raw == fast_provider_raw
    {
        tracing::info!(
            model = %strong_model,
            provider = strong_provider_raw.as_deref().unwrap_or("(auto)"),
            session_id = %session_id,
            "single-model LLM (set fast model + provider for dual routing)"
        );
        Box::new(strong)
    } else {
        tracing::info!(
            fast = %fast_model,
            fast_provider = fast_provider_raw.as_deref().unwrap_or("(auto)"),
            strong = %strong_model,
            strong_provider = strong_provider_raw.as_deref().unwrap_or("(auto)"),
            pin_provider = pin,
            session_id = %session_id,
            "dual-model routing enabled"
        );
        Box::new(boris_agent::RoutingClient::new(
            Box::new(fast),
            Box::new(strong),
        ))
    };
    let mut agent = Agent::new(client, &config.system_prompt);
    paths::migrate_home_if_needed();
    if let Err(e) = paths::ensure_agent_dirs() {
        tracing::warn!(error = %e, "ensure agent workspace/audit dirs failed");
    }

    let preset = config.capability_preset;
    let mut sandbox = SandboxConfig::for_desktop_mvp(paths::boris_home());
    preset.apply_to_sandbox(&mut sandbox);
    // Trusted auto-allow for Moderate tools (notes, workspace writes, clipboard…).
    // Dangerous (bash, open url) still confirm. Disable with BORIS_TRUSTED=0.
    let trusted = std::env::var("BORIS_TRUSTED")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v == "0" || v == "false" || v == "off" || v == "no")
        })
        .unwrap_or(true);
    sandbox = sandbox.with_trusted_auto_moderate(trusted);
    agent.configure_runtime(sandbox, Some(paths::audit_path()));

    // Core + (optional) power tools filtered by capability preset + personal context.
    let power = preset.wants_power_tools();
    boris_agent::tools::register_builtin_tools_with_preset(
        &mut agent,
        boris_agent::tools::BuiltinToolPaths {
            notes_path: paths::notes_path(),
            profile_path: paths::profile_path(),
            sandbox_root: paths::workspace_dir(),
            data_roots: vec![
                paths::memory_dir(),
                paths::sessions_root(),
                paths::workspace_dir(),
            ],
            allow_read: boris_agent::default_user_read_roots(),
            allow_write: vec![],
            boris_home: paths::boris_home(),
        },
        true,
        power,
        preset,
    );

    // Skills: install defaults into ~/.boris/skills if missing, then enable catalog + tools.
    match boris_agent::ensure_default_skills(&paths::boris_home()) {
        Ok(written) if !written.is_empty() => {
            tracing::info!(count = written.len(), "installed default skill playbooks");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "ensure default skills failed"),
    }
    let cwd = std::env::current_dir().ok();
    let loaded = boris_agent::load_skills(cwd.as_deref(), &paths::boris_home(), &[], true);
    let skill_count = loaded.skills.len();
    agent.enable_skills(loaded);

    if config.long_term_memory {
        match agent.enable_long_term_memory(paths::memory_dir()) {
            Ok(_) => tracing::info!(
                memory_md = %paths::memory_md_path().display(),
                "long-term markdown memory enabled"
            ),
            Err(e) => tracing::warn!(error = %e, "long-term memory enable failed"),
        }
    }

    // P3 lean subagent (read-mostly child loop).
    agent.enable_subagents();

    // Tool runtime flags. Defaults: wave scheduling on (parallel read wave),
    // progressive listing opt-in.
    let mut features = boris_agent::ToolRuntimeFeatures::default();
    if env_flag_true("BORIS_PROGRESSIVE_TOOLS") {
        features.progressive_listing = true;
    }
    // Default on. Opt out with BORIS_WAVE_SCHEDULING=0 (or legacy BORIS_CONCURRENCY_V2=0).
    if env_flag_false("BORIS_WAVE_SCHEDULING") || env_flag_false("BORIS_CONCURRENCY_V2") {
        features.wave_scheduling = false;
    }
    if env_flag_false("BORIS_PROGRESS_EVENTS") {
        features.progress_events = false;
    }
    if let Ok(n) = std::env::var("BORIS_MAX_PARALLEL_TOOLS") {
        if let Ok(parsed) = n.trim().parse::<u32>() {
            if parsed >= 1 {
                features.max_parallel_tools = parsed;
            }
        }
    }
    agent.set_features(features.clone());
    if features.progressive_listing {
        agent.ensure_tool_search();
    }

    tracing::info!(
        notes = %paths::notes_path().display(),
        profile = %paths::profile_path().display(),
        workspace = %paths::workspace_dir().display(),
        audit = %paths::audit_path().display(),
        skills = skill_count,
        skills_dir = %paths::skills_dir().display(),
        capability = preset.as_str(),
        long_term_memory = config.long_term_memory,
        trusted_auto_moderate = trusted,
        progressive_listing = features.progressive_listing,
        wave_scheduling = features.wave_scheduling,
        max_parallel_tools = features.max_parallel_tools,
        progress_events = features.progress_events,
        "builtin tools + skills + memory + subagent + tool runtime registered"
    );

    agent
}
