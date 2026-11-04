pub mod manifest;
pub mod runtime;

use manifest::{list_adapter_dirs, load_adapter_dir, AdapterInfo, LoadedAdapter};
use tauri::{AppHandle, Manager};

pub const MOCK_ADAPTER_ID: &str = "mock";

/// Return the adapter package root. In a bundled app Tauri resources contain
/// `adapters/`; in development we use the repository directory so the runtime
/// can be tested without rebuilding installers.
pub fn adapter_root(app: &AppHandle) -> Option<std::path::PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join("adapters");
        if bundled.is_dir() {
            return Some(bundled);
        }
    }
    let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../adapters");
    source.is_dir().then_some(source)
}

pub fn list_builtin(app: &AppHandle) -> Vec<AdapterInfo> {
    adapter_root(app)
        .map(|root| list_adapter_dirs(&root, "builtin"))
        .unwrap_or_default()
}

pub fn load_mock(app: &AppHandle) -> Result<LoadedAdapter, String> {
    let root = adapter_root(app).ok_or_else(|| "找不到适配器目录".to_string())?;
    load_adapter_dir(&root.join(MOCK_ADAPTER_ID), "builtin")
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AdapterSmokeResult {
    pub adapter_id: String,
    pub adapter_version: String,
    pub complete: bool,
    pub task_count: usize,
    pub task_titles: Vec<String>,
}
