pub mod catalog;
pub mod install;
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

fn load_candidates_from_root(root: &std::path::Path) -> Vec<LoadedAdapter> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_adapter_dir(&path, "installed") {
            Ok(adapter) => loaded.push(adapter),
            Err(_) => {}
        }
    }
    loaded
}

/// Find the best compatible adapter package for an id. Installed packages
/// with a higher sequence win over built-in packages.
pub fn load_by_id(app: &AppHandle, id: &str) -> Option<LoadedAdapter> {
    let mut candidates = Vec::new();
    if let Some(root) = adapter_root(app) {
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(adapter) = load_adapter_dir(&path, "builtin") {
                    candidates.push(adapter);
                }
            }
        }
    }
    if let Ok(installed) = install::installed_root(app) {
        candidates.extend(load_candidates_from_root(&installed));
    }
    candidates
        .into_iter()
        .filter(|adapter| adapter.manifest.id == id)
        .max_by_key(|adapter| adapter.manifest.sequence.unwrap_or(0))
}

pub fn list_all(app: &AppHandle) -> Vec<AdapterInfo> {
    let mut adapters = list_builtin(app);
    if let Ok(root) = install::installed_root(app) {
        adapters.extend(list_adapter_dirs(&root, "installed"));
    }
    adapters.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| left.version.cmp(&right.version))
            .then_with(|| left.source.cmp(&right.source))
    });
    adapters
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
