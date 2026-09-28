/// Catalog of visual cards in the active voice session.
#[tauri::command]
pub async fn list_session_artifacts() -> Result<Vec<boris_pipeline::ArtifactListItem>, String> {
    tauri::async_runtime::spawn_blocking(crate::artifacts::list_current)
        .await
        .map_err(|e| format!("list_session_artifacts join: {e}"))?
}

/// Body + meta for one session card (`id` omitted → current).
#[tauri::command]
pub async fn get_session_artifact(
    id: Option<String>,
) -> Result<boris_pipeline::ArtifactCard, String> {
    tauri::async_runtime::spawn_blocking(move || crate::artifacts::get_current(id))
        .await
        .map_err(|e| format!("get_session_artifact join: {e}"))?
}
