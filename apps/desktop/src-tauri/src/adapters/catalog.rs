use super::install::{install_package, trusted_key};
use super::manifest::{AdapterInfo, CURRENT_API_VERSION};
use super::load_by_id;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tauri::AppHandle;

const CATALOG_FORMAT: &str = "study-deadline-adapter-catalog/1";
const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/DongGuZhengHuaJi/study-deadline/main/adapters/catalog.json";
const MAX_CATALOG_BYTES: usize = 512 * 1024;
const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEnvelope {
    format: String,
    key_id: String,
    generated_at: String,
    adapters_json: String,
    signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub sequence: u64,
    pub url: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateReport {
    pub installed: Vec<AdapterInfo>,
    pub up_to_date: usize,
    pub catalog_url: String,
}

fn sha256_b64(bytes: &[u8]) -> String {
    STANDARD.encode(Sha256::digest(bytes))
}

fn catalog_payload(envelope: &CatalogEnvelope) -> String {
    format!(
        "{CATALOG_FORMAT}\n{}\n{}\n{}",
        envelope.key_id,
        sha256_b64(envelope.generated_at.as_bytes()),
        sha256_b64(envelope.adapters_json.as_bytes())
    )
}

fn verify_catalog_with_key(
    contents: &str,
    verifying_key: &ed25519_dalek::VerifyingKey,
) -> Result<Vec<CatalogEntry>, String> {
    if contents.len() > MAX_CATALOG_BYTES {
        return Err("适配器目录过大".into());
    }
    let envelope: CatalogEnvelope =
        serde_json::from_str(contents).map_err(|error| format!("目录 JSON 无效: {error}"))?;
    if envelope.format != CATALOG_FORMAT {
        return Err(format!("不支持的适配器目录格式: {}", envelope.format));
    }
    use ed25519_dalek::{Signature, Verifier};
    let signature_bytes = STANDARD
        .decode(&envelope.signature)
        .map_err(|error| format!("目录签名不是合法 base64: {error}"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| "目录签名长度无效".to_string())?;
    verifying_key
        .verify(catalog_payload(&envelope).as_bytes(), &signature)
        .map_err(|_| "适配器目录签名验证失败".to_string())?;
    let entries: Vec<CatalogEntry> = serde_json::from_str(&envelope.adapters_json)
        .map_err(|error| format!("目录 adaptersJson 无效: {error}"))?;
    if entries.len() > 100 {
        return Err("目录适配器数量过多".into());
    }
    for entry in &entries {
        if entry.id.is_empty() || entry.version.is_empty() || entry.api_version != CURRENT_API_VERSION {
            return Err("目录包含不兼容的适配器条目".into());
        }
        if !entry.url.starts_with("https://") {
            return Err("目录适配器 URL 必须使用 HTTPS".into());
        }
        if entry.sha256.is_empty() || entry.sequence == 0 {
            return Err("目录条目缺少 sha256 或 sequence".into());
        }
    }
    Ok(entries)
}

fn fetch(client: &reqwest::blocking::Client, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("下载失败: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("下载失败: HTTP {}", response.status()));
    }
    if let Some(length) = response.content_length() {
        if length as usize > max_bytes {
            return Err("下载内容过大".into());
        }
    }
    let bytes = response
        .bytes()
        .map_err(|error| format!("读取下载内容失败: {error}"))?;
    if bytes.len() > max_bytes {
        return Err("下载内容过大".into());
    }
    Ok(bytes.to_vec())
}

/// Fetch, verify and install newer signed adapter packages from the catalog.
/// The catalog signature and every package signature are both verified.
pub fn check_and_install(app: &AppHandle) -> Result<UpdateReport, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("StudyDeadline/adapter-updater")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("创建更新客户端失败: {error}"))?;

    let catalog_bytes = fetch(&client, DEFAULT_CATALOG_URL, MAX_CATALOG_BYTES)?;
    let catalog_text = String::from_utf8(catalog_bytes)
        .map_err(|error| format!("目录不是 UTF-8: {error}"))?;
    let envelope: CatalogEnvelope =
        serde_json::from_str(&catalog_text).map_err(|error| format!("目录 JSON 无效: {error}"))?;
    let verifying_key = trusted_key(&envelope.key_id)?;
    let entries = verify_catalog_with_key(&catalog_text, &verifying_key)?;

    let mut installed = Vec::new();
    let mut up_to_date = 0usize;
    for entry in entries {
        let current_sequence = load_by_id(app, &entry.id)
            .and_then(|adapter| adapter.manifest.sequence)
            .unwrap_or(0);
        if current_sequence >= entry.sequence {
            up_to_date += 1;
            continue;
        }
        let bytes = fetch(&client, &entry.url, MAX_PACKAGE_BYTES)?;
        let actual_hash = sha256_b64(&bytes);
        if actual_hash != entry.sha256 {
            return Err(format!("适配器 {} 下载校验失败", entry.id));
        }
        let package = String::from_utf8(bytes)
            .map_err(|error| format!("适配器包不是 UTF-8: {error}"))?;
        let info = install_package(app, &package)?;
        installed.push(info);
    }

    Ok(UpdateReport {
        installed,
        up_to_date,
        catalog_url: DEFAULT_CATALOG_URL.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const ENTRIES: &str =
        r#"[{"id":"mock","name":"Mock","version":"0.1.1","apiVersion":1,"sequence":2,"url":"https://example.com/mock.sdapkg","sha256":"abc"}]"#;

    fn envelope(key_id: &str, generated_at: &str, signing_key: &SigningKey) -> String {
        let mut base = CatalogEnvelope {
            format: CATALOG_FORMAT.into(),
            key_id: key_id.into(),
            generated_at: generated_at.into(),
            adapters_json: ENTRIES.into(),
            signature: String::new(),
        };
        let signature = signing_key.sign(catalog_payload(&base).as_bytes());
        base.signature = STANDARD.encode(signature.to_bytes());
        serde_json::to_string(&base).unwrap()
    }

    #[test]
    fn verifies_signed_catalog() {
        let signing = SigningKey::from_bytes(&[11u8; 32]);
        let contents = envelope("test-key", "2026-11-04T00:00:00Z", &signing);
        let entries =
            verify_catalog_with_key(&contents, &signing.verifying_key()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].sequence, 2);
    }

    #[test]
    fn verifies_repository_catalog_fixture() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapters/catalog.json");
        let contents = std::fs::read_to_string(path).unwrap();
        let entries = verify_catalog_with_key(&contents, &trusted_key("dev-2026").unwrap()).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|entry| entry.id == "yuketang"));
        assert!(entries.iter().any(|entry| entry.id == "chaoxing"));

        // Every repository package must match the hash advertised by the
        // signed catalog, otherwise the app would reject it after download.
        let packages = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapter-packages");
        for entry in &entries {
            let path = packages.join(format!("{}-{}.sdapkg", entry.id, entry.version));
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()));
            assert_eq!(sha256_b64(&bytes), entry.sha256, "{} 哈希不匹配", entry.id);
        }
    }

    #[test]
    fn rejects_tampered_catalog() {
        let signing = SigningKey::from_bytes(&[11u8; 32]);
        let contents = envelope("test-key", "2026-11-04T00:00:00Z", &signing);
        let mut value: serde_json::Value = serde_json::from_str(&contents).unwrap();
        value["generatedAt"] = serde_json::Value::String("2026-11-05T00:00:00Z".into());
        assert!(verify_catalog_with_key(
            &value.to_string(),
            &signing.verifying_key()
        )
        .is_err());
    }
}
