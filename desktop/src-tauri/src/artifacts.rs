//! Thin host adapters over [`boris_pipeline`] session-card helpers.

pub fn list_current() -> Result<Vec<boris_pipeline::ArtifactListItem>, String> {
    boris_pipeline::list_session_artifacts()
}

pub fn get_current(id: Option<String>) -> Result<boris_pipeline::ArtifactCard, String> {
    boris_pipeline::get_session_artifact(id.as_deref())
}
