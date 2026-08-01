mod orchestrator;
mod overlay_win;

use orchestrator::AppState;
use boris_pipeline::{DeviceDto, StatusPicture};
use tauri::{AppHandle, Emitter, Manager, State};

fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into())
                .add_directive("boris_pipeline=info".parse().unwrap())
                .add_directive("boris_audio=info".parse().unwrap())
                .add_directive("boris_sense=info".parse().unwrap())
                .add_directive("boris_desktop=info".parse().unwrap()),
        )
        .try_init();
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> StatusPicture {
    state.status()
}

#[tauri::command]
fn start_engine(
    app: AppHandle,
    state: State<'_, AppState>,
    api_key: String,
    model: Option<String>,
) -> Result<(), String> {
    let key = if api_key.trim().is_empty() {
        std::env::var("OPENROUTER_API_KEY").unwrap_or_default()
    } else {
        api_key
    };
    let model = model.or_else(|| std::env::var("OPENROUTER_MODEL").ok());

    state.start(key, model, move |picture| {
        let _ = app.emit("status", picture);
    })
}

#[tauri::command]
fn stop_engine(state: State<'_, AppState>) -> Result<(), String> {
    state.stop()
}

#[tauri::command]
fn list_input_devices() -> Vec<DeviceDto> {
    AppState::list_inputs()
}

#[tauri::command]
fn list_output_devices() -> Vec<DeviceDto> {
    AppState::list_outputs()
}

#[tauri::command]
fn switch_input(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    state.switch_input(device_id)
}

#[tauri::command]
fn switch_output(state: State<'_, AppState>, device_id: String) -> Result<(), String> {
    state.switch_output(device_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_status,
            start_engine,
            stop_engine,
            list_input_devices,
            list_output_devices,
            switch_input,
            switch_output,
        ])
        .setup(|app| {
            // Overlay is `"create": false` in config — build with explicit transparent API.
            overlay_win::spawn_overlay_window(app.handle())?;

            if let Some(state) = app.try_state::<AppState>() {
                let _ = app.emit("status", state.status());
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
