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

/// Installed packages use `<root>/<adapter-id>/<version>/manifest.json`.
/// Return each version directory so discovery checks every installed version.
fn installed_version_dirs(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let adapter_dir = entry.path();
        if !adapter_dir.is_dir() {
            continue;
        }
        let Ok(versions) = std::fs::read_dir(&adapter_dir) else {
            continue;
        };
        for version in versions.flatten() {
            let path = version.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }
    dirs
}

fn load_candidates_from_root(root: &std::path::Path) -> Vec<LoadedAdapter> {
    installed_version_dirs(root)
        .into_iter()
        .filter_map(|path| load_adapter_dir(&path, "installed").ok())
        .collect()
}

fn list_installed(root: &std::path::Path) -> Vec<AdapterInfo> {
    installed_version_dirs(root)
        .into_iter()
        .filter_map(|path| {
            load_adapter_dir(&path, "installed")
                .ok()
                .map(|adapter| AdapterInfo {
                    id: adapter.manifest.id,
                    name: adapter.manifest.name,
                    version: adapter.manifest.version,
                    api_version: adapter.manifest.api_version,
                    source: "installed".into(),
                    path: path.display().to_string(),
                })
        })
        .collect()
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
        adapters.extend(list_installed(&root));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_installed_adapter(root: &std::path::Path, id: &str, version: &str, sequence: u64) {
        let dir = root.join(id).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(
                r#"{{
                  "id": "{id}",
                  "name": "{id}",
                  "version": "{version}",
                  "apiVersion": 1,
                  "entry": "index.js",
                  "keyId": "dev-2026",
                  "sequence": {sequence},
                  "permissions": {{ "http": [], "session": [] }}
                }}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("index.js"),
            "globalThis.adapter={fetchTasks:()=>({tasks:[],complete:true})};",
        )
        .unwrap();
    }

    #[test]
    fn discovers_versioned_installed_adapters() {
        let root = std::env::temp_dir().join(format!(
            "study-deadline-installed-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        write_installed_adapter(&root, "demo", "0.1.0", 1);
        write_installed_adapter(&root, "demo", "0.1.1", 2);

        let loaded = load_candidates_from_root(&root);
        assert_eq!(loaded.len(), 2);
        let listed = list_installed(&root);
        assert_eq!(listed.len(), 2);
        let best = loaded
            .into_iter()
            .max_by_key(|adapter| adapter.manifest.sequence.unwrap_or(0))
            .unwrap();
        assert_eq!(best.manifest.version, "0.1.1");

        let _ = std::fs::remove_dir_all(&root);
    }
}
