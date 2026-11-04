use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CURRENT_API_VERSION: u32 = 1;
const MAX_ENTRY_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterManifest {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_api_version")]
    pub api_version: u32,
    #[serde(default)]
    pub min_host_version: Option<String>,
    #[serde(default)]
    pub max_host_version: Option<String>,
    pub entry: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: AdapterPermissions,
    #[serde(default)]
    pub auth: Option<AdapterAuth>,
    #[serde(default)]
    pub key_id: Option<String>,
    #[serde(default)]
    pub sequence: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterPermissions {
    #[serde(default)]
    pub http: Vec<String>,
    #[serde(default)]
    pub session: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterAuth {
    pub login_url: String,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub success_url_patterns: Vec<String>,
    #[serde(default)]
    pub session_cookies: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct LoadedAdapter {
    pub manifest: AdapterManifest,
    #[allow(dead_code)]
    pub root: PathBuf,
    pub entry_source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AdapterInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub source: String,
    pub path: String,
}

fn default_schema_version() -> u32 {
    1
}

fn default_api_version() -> u32 {
    1
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && id
            .bytes()
            .next()
            .map(|byte| byte.is_ascii_alphanumeric())
            .unwrap_or(false)
}

impl AdapterManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "适配器 schemaVersion 不支持: {}",
                self.schema_version
            ));
        }
        if !valid_id(&self.id) {
            return Err("适配器 id 必须是 1-64 位小写字母/数字/连字符".into());
        }
        if self.name.trim().is_empty() || self.name.len() > 120 {
            return Err("适配器 name 为空或过长".into());
        }
        if self.version.trim().is_empty() || self.version.len() > 64 {
            return Err("适配器 version 为空或过长".into());
        }
        if self.api_version != CURRENT_API_VERSION {
            return Err(format!(
                "适配器 apiVersion {} 与宿主不兼容（需要 {}）",
                self.api_version, CURRENT_API_VERSION
            ));
        }
        if self.entry.is_empty()
            || self.entry.starts_with('/')
            || self.entry.starts_with('\\')
            || self.entry.contains("..")
            || self.entry.contains('/')
            || self.entry.contains('\\')
            || !self.entry.ends_with(".js")
        {
            return Err("适配器 entry 必须是包内单个 .js 文件名".into());
        }
        for pattern in &self.permissions.http {
            if !pattern.starts_with("https://") {
                return Err(format!("HTTP 权限只允许 HTTPS: {pattern}"));
            }
        }
        if let Some(auth) = &self.auth {
            if !auth.login_url.starts_with("https://") {
                return Err("auth.loginUrl 必须是 HTTPS".into());
            }
        }
        Ok(())
    }
}

pub fn load_adapter_dir(root: &Path, _source: &str) -> Result<LoadedAdapter, String> {
    let manifest_path = root.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("读取 {} 失败: {error}", manifest_path.display()))?;
    if raw.len() > 512 * 1024 {
        return Err("manifest.json 过大".into());
    }
    let manifest: AdapterManifest = serde_json::from_str(&raw)
        .map_err(|error| format!("{} 不是合法 manifest: {error}", manifest_path.display()))?;
    manifest.validate()?;

    let entry_path = root.join(&manifest.entry);
    let metadata = std::fs::metadata(&entry_path)
        .map_err(|error| format!("读取适配器 entry 失败: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
        return Err("适配器 entry 不是文件或体积过大".into());
    }
    let entry_source = std::fs::read_to_string(&entry_path)
        .map_err(|error| format!("读取适配器 entry 失败: {error}"))?;

    Ok(LoadedAdapter {
        manifest,
        root: root.to_path_buf(),
        entry_source,
    })
}

pub fn list_adapter_dirs(root: &Path, source: &str) -> Vec<AdapterInfo> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut adapters = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match load_adapter_dir(&path, source) {
            Ok(adapter) => adapters.push(AdapterInfo {
                id: adapter.manifest.id,
                name: adapter.manifest.name,
                version: adapter.manifest.version,
                api_version: adapter.manifest.api_version,
                source: source.to_string(),
                path: path.display().to_string(),
            }),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "invalid adapter package");
            }
        }
    }
    adapters.sort_by(|left, right| left.id.cmp(&right.id));
    adapters
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mock_manifest() -> AdapterManifest {
        serde_json::from_str(
            r#"{
              "id": "mock",
              "name": "Mock",
              "version": "0.1.0",
              "entry": "index.js",
              "permissions": { "http": [], "session": [] }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn validates_mock_manifest() {
        mock_manifest().validate().unwrap();
    }

    #[test]
    fn rejects_traversal_entry() {
        let mut manifest = mock_manifest();
        manifest.entry = "../index.js".into();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn rejects_plain_http_permission() {
        let mut manifest = mock_manifest();
        manifest.permissions.http.push("http://example.com/*".into());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn loads_repo_mock_adapter() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../adapters/mock");
        let loaded = load_adapter_dir(&root, "builtin").unwrap();
        assert_eq!(loaded.manifest.id, "mock");
        assert!(loaded.entry_source.contains("fetchTasks"));
    }
}
