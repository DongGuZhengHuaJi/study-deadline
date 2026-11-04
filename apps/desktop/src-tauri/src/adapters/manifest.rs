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
    /// Optional platform base URL used as `ctx.config.baseUrl` and as the
    /// account's stored base URL. Falls back to the login URL origin.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub success_url_patterns: Vec<String>,
    /// Cookie names to copy into the account session.
    #[serde(default)]
    pub session_cookies: Vec<String>,
    /// At least one of these cookies must be present after login. When empty,
    /// any cookie from the adapter's allowed HTTP domains is accepted.
    #[serde(default)]
    pub required_any_cookies: Vec<String>,
}

impl AdapterAuth {
    /// Base URL stored on a session and exposed as `ctx.config.baseUrl`.
    pub fn resolved_base_url(&self) -> Result<String, String> {
        if let Some(base) = self.base_url.as_deref() {
            let parsed = url::Url::parse(base)
                .map_err(|error| format!("auth.baseUrl 无效: {error}"))?;
            if parsed.scheme() != "https" {
                return Err("auth.baseUrl 必须是 HTTPS".into());
            }
            return Ok(parsed.origin().ascii_serialization());
        }
        let parsed = url::Url::parse(&self.login_url)
            .map_err(|error| format!("auth.loginUrl 无效: {error}"))?;
        Ok(parsed.origin().ascii_serialization())
    }

    /// Domains whose cookies may be extracted after login. Derived from the
    /// adapter's HTTP permission host patterns so school SSO/idP cookies are
    /// never copied into the platform session.
    pub fn cookie_domains(&self, http_permissions: &[String]) -> Vec<String> {
        let mut domains = Vec::new();
        for pattern in http_permissions {
            let Some(rest) = pattern.strip_prefix("https://") else {
                continue;
            };
            let host = rest.split('/').next().unwrap_or("");
            let host = host.trim_start_matches("*.");
            if host.is_empty() {
                continue;
            }
            let domain = host.to_ascii_lowercase();
            if !domains.contains(&domain) {
                domains.push(domain);
            }
        }
        domains
    }
}

/// True when a cookie domain belongs to one of the adapter's HTTP hosts.
///
/// Both directions are accepted: a cookie set on `.yuketang.cn` covers
/// `www.yuketang.cn`, while a cookie set on a specific subdomain also matches
/// a broader permission pattern. The dot check prevents `evil-example.com`
/// from matching `example.com`.
pub fn cookie_domain_allowed(domain: Option<&str>, allowed: &[String]) -> bool {
    let Some(domain) = domain else {
        // A missing domain cannot be attributed to the platform. Callers may
        // still accept host-only cookies whose names are explicitly declared
        // in the manifest, but the domain filter itself stays conservative so
        // school IdP cookies are never copied into a platform session.
        return false;
    };
    let domain = domain.trim_start_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return false;
    }
    allowed.iter().any(|allowed| {
        let allowed = allowed.trim_start_matches('.').to_ascii_lowercase();
        domain == allowed
            || domain.ends_with(&format!(".{allowed}"))
            || allowed.ends_with(&format!(".{domain}"))
    })
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
        if self.version.trim().is_empty()
            || self.version.len() > 64
            || !self.version.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'+'
            })
        {
            return Err("适配器 version 为空、过长或包含路径字符".into());
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
            if let Some(base_url) = &auth.base_url {
                if !base_url.starts_with("https://") {
                    return Err("auth.baseUrl 必须是 HTTPS".into());
                }
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

    #[test]
    fn cookie_domain_filter_uses_http_permissions() {
        let auth: AdapterAuth = serde_json::from_str(
            r#"{
              "loginUrl": "https://passport2.chaoxing.com/login",
              "requiredAnyCookies": ["_uid", "UID"]
            }"#,
        )
        .unwrap();
        let domains = auth.cookie_domains(&["https://*.chaoxing.com/*".to_string()]);
        assert!(cookie_domain_allowed(Some(".chaoxing.com"), &domains));
        assert!(cookie_domain_allowed(Some("i.chaoxing.com"), &domains));
        assert!(!cookie_domain_allowed(
            Some("authserver.eitech.edu.cn"),
            &domains
        ));
        assert!(!cookie_domain_allowed(Some("evil-chaoxing.com"), &domains));
        assert!(!cookie_domain_allowed(None, &domains));
    }

    #[test]
    fn yuketang_apex_cookie_matches_region_hosts() {
        let domains = vec![
            "www.yuketang.cn".to_string(),
            "pro.yuketang.cn".to_string(),
            "changjiang.yuketang.cn".to_string(),
            "huanghe.yuketang.cn".to_string(),
        ];
        assert!(cookie_domain_allowed(Some(".yuketang.cn"), &domains));
        assert!(!cookie_domain_allowed(Some("evil-yuketang.cn"), &domains));
    }

    #[test]
    fn auth_base_url_defaults_to_login_origin() {
        let auth: AdapterAuth = serde_json::from_str(
            r#"{"loginUrl": "https://passport2.chaoxing.com/login?fid="}"#,
        )
        .unwrap();
        assert_eq!(
            auth.resolved_base_url().unwrap(),
            "https://passport2.chaoxing.com"
        );
        let explicit: AdapterAuth = serde_json::from_str(
            r#"{"loginUrl": "https://passport2.chaoxing.com/login", "baseUrl": "https://i.chaoxing.com"}"#,
        )
        .unwrap();
        assert_eq!(
            explicit.resolved_base_url().unwrap(),
            "https://i.chaoxing.com"
        );
    }
}
