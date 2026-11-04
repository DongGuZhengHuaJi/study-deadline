use super::manifest::{load_adapter_dir, AdapterInfo, AdapterManifest};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const ENVELOPE_FORMAT: &str = "study-deadline-adapter/1";
const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;

/// Development and first-party adapter signing keys. Rotate these before a
/// public production release; the private key must never enter this repo.
const TRUSTED_KEYS: &[(&str, &str)] = &[(
    "dev-2026",
    "3J2vUsrfhJDWkpF9OI3SyQW0QtBYhRj2u5/MH9klXFs=",
)];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdapterEnvelope {
    format: String,
    key_id: String,
    manifest_json: String,
    source: String,
    signature: String,
}

pub fn installed_root(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("无法定位应用数据目录: {error}"))?
        .join("adapters");
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("创建适配器目录失败: {error}"))?;
    Ok(root)
}

pub(crate) fn trusted_key(key_id: &str) -> Result<VerifyingKey, String> {
    let (_, encoded) = TRUSTED_KEYS
        .iter()
        .find(|(id, _)| *id == key_id)
        .ok_or_else(|| format!("未知的适配器签名密钥: {key_id}"))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| format!("内置签名公钥损坏: {error}"))?;
    let key_bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "内置签名公钥长度错误".to_string())?;
    VerifyingKey::from_bytes(&key_bytes).map_err(|error| format!("签名公钥无效: {error}"))
}

fn sha256_b64(data: &[u8]) -> String {
    STANDARD.encode(Sha256::digest(data))
}

fn signature_payload(key_id: &str, manifest_json: &str, source: &str) -> String {
    format!(
        "{ENVELOPE_FORMAT}\n{key_id}\n{}\n{}",
        sha256_b64(manifest_json.as_bytes()),
        sha256_b64(source.as_bytes())
    )
}

fn parse_and_verify(
    contents: &str,
    key_for_id: impl Fn(&str) -> Result<VerifyingKey, String>,
) -> Result<(AdapterManifest, AdapterEnvelope), String> {
    if contents.len() > MAX_PACKAGE_BYTES {
        return Err("适配器包过大".into());
    }
    let envelope: AdapterEnvelope =
        serde_json::from_str(contents).map_err(|error| format!("适配器包 JSON 无效: {error}"))?;
    if envelope.format != ENVELOPE_FORMAT {
        return Err(format!("不支持的适配器包格式: {}", envelope.format));
    }
    if envelope.source.len() > MAX_SOURCE_BYTES {
        return Err("适配器源码过大".into());
    }
    if envelope.manifest_json.len() > 512 * 1024 {
        return Err("适配器 manifest 过大".into());
    }
    let verifying_key = key_for_id(&envelope.key_id)?;
    let signature_bytes = STANDARD
        .decode(&envelope.signature)
        .map_err(|error| format!("签名不是合法 base64: {error}"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| "签名长度无效".to_string())?;
    let payload = signature_payload(&envelope.key_id, &envelope.manifest_json, &envelope.source);
    verifying_key
        .verify(payload.as_bytes(), &signature)
        .map_err(|_| "适配器签名验证失败".to_string())?;

    let manifest: AdapterManifest = serde_json::from_str(&envelope.manifest_json)
        .map_err(|error| format!("适配器 manifest 无效: {error}"))?;
    manifest.validate()?;
    if manifest.sequence.unwrap_or(0) == 0 {
        return Err("签名适配器 manifest 必须提供正整数 sequence".into());
    }
    if let Some(key_id) = &manifest.key_id {
        if key_id != &envelope.key_id {
            return Err("manifest.keyId 与签名 keyId 不一致".into());
        }
    } else {
        return Err("签名适配器 manifest 必须提供 keyId".into());
    }
    Ok((manifest, envelope))
}

fn max_installed_sequence(adapter_dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(adapter_dir) else {
        return 0;
    };
    let mut max = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path.join("manifest.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<AdapterManifest>(&raw) else {
            continue;
        };
        max = max.max(manifest.sequence.unwrap_or(0));
    }
    max
}

/// Verify and install a local signed adapter package into app data.
pub fn install_package(app: &AppHandle, contents: &str) -> Result<AdapterInfo, String> {
    let root = installed_root(app)?;
    install_package_into(&root, contents)
}

fn install_package_into(root: &Path, contents: &str) -> Result<AdapterInfo, String> {
    let (manifest, envelope) = parse_and_verify(contents, trusted_key)?;
    let root = root.to_path_buf();
    let adapter_dir = root.join(&manifest.id);
    let target = adapter_dir.join(&manifest.version);
    if target.exists() {
        return Err(format!(
            "适配器 {} 版本 {} 已安装",
            manifest.id, manifest.version
        ));
    }
    let current_sequence = max_installed_sequence(&adapter_dir);
    let sequence = manifest.sequence.unwrap_or(0);
    if current_sequence > 0 && sequence <= current_sequence {
        return Err(format!(
            "拒绝降级：当前 sequence={current_sequence}，包 sequence={sequence}"
        ));
    }

    std::fs::create_dir_all(&adapter_dir)
        .map_err(|error| format!("创建适配器目录失败: {error}"))?;
    let temp_dir = root.join(format!(".install-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|error| format!("创建临时目录失败: {error}"))?;
    let write_result = (|| -> Result<(), String> {
        std::fs::write(temp_dir.join("manifest.json"), envelope.manifest_json.as_bytes())
            .map_err(|error| format!("写入 manifest 失败: {error}"))?;
        std::fs::write(temp_dir.join("index.js"), envelope.source.as_bytes())
            .map_err(|error| format!("写入适配器源码失败: {error}"))?;
        std::fs::write(temp_dir.join("package.sdapkg"), contents.as_bytes())
            .map_err(|error| format!("写入签名包失败: {error}"))?;
        load_adapter_dir(&temp_dir, "installed")?;
        std::fs::rename(&temp_dir, &target)
            .map_err(|error| format!("安装适配器失败（目标可能被占用）: {error}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(write_result.unwrap_err());
    }

    let path = target.display().to_string();
    Ok(AdapterInfo {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        api_version: manifest.api_version,
        source: "installed".into(),
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    const MANIFEST: &str = r#"{
      "id":"signed-mock","name":"Signed Mock","version":"0.1.0",
      "apiVersion":1,"entry":"index.js","keyId":"test-key","sequence":1,
      "permissions":{"http":[],"session":[]}
    }"#;
    const SOURCE: &str = "globalThis.adapter={fetchTasks:()=>({tasks:[],complete:true})};";

    fn envelope_for(key_id: &str, signing_key: &SigningKey, source: &str) -> String {
        let payload = signature_payload(key_id, MANIFEST, source);
        let signature = signing_key.sign(payload.as_bytes());
        serde_json::json!({
            "format": ENVELOPE_FORMAT,
            "keyId": key_id,
            "manifestJson": MANIFEST,
            "source": source,
            "signature": STANDARD.encode(signature.to_bytes()),
        })
        .to_string()
    }

    fn test_key() -> (SigningKey, VerifyingKey) {
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let verifying = signing.verifying_key();
        (signing, verifying)
    }

    #[test]
    fn verifies_valid_envelope() {
        let (signing, verifying) = test_key();
        let contents = envelope_for("test-key", &signing, SOURCE);
        let (manifest, _) = parse_and_verify(&contents, |_| Ok(verifying)).unwrap();
        assert_eq!(manifest.id, "signed-mock");
    }

    #[test]
    fn rejects_tampered_source() {
        let (signing, verifying) = test_key();
        let contents = envelope_for("test-key", &signing, SOURCE);
        let mut envelope: serde_json::Value = serde_json::from_str(&contents).unwrap();
        envelope["source"] = serde_json::Value::String("globalThis.adapter={};".into());
        let tampered = envelope.to_string();
        assert!(parse_and_verify(&tampered, |_| Ok(verifying)).is_err());
    }

    #[test]
    fn verifies_dev_signed_mock_package() {
        // This fixture is signed with the development key whose public half is
        // in TRUSTED_KEYS. It is safe to commit: only the public key is here.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapters/mock/mock-0.1.0.sdapkg");
        let contents = std::fs::read_to_string(path).unwrap();
        let (manifest, _) = parse_and_verify(&contents, trusted_key).unwrap();
        assert_eq!(manifest.id, "mock");
        assert_eq!(manifest.sequence, Some(1));
    }

    #[test]
    fn verifies_dev_signed_chaoxing_package() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapter-packages/chaoxing-0.1.0.sdapkg");
        let contents = std::fs::read_to_string(path).unwrap();
        let (manifest, _) = parse_and_verify(&contents, trusted_key).unwrap();
        assert_eq!(manifest.id, "chaoxing");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.sequence, Some(1));
        assert!(manifest.auth.is_some());
    }

    #[test]
    fn installs_signed_package_and_rejects_duplicate() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapters/mock/mock-0.1.0.sdapkg");
        let contents = std::fs::read_to_string(path).unwrap();
        let root = std::env::temp_dir().join(format!("study-deadline-install-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let installed = install_package_into(&root, &contents).unwrap();
        assert_eq!(installed.id, "mock");
        assert!(PathBuf::from(&installed.path).join("index.js").is_file());
        assert!(install_package_into(&root, &contents).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unknown_key() {
        let (signing, _) = test_key();
        let contents = envelope_for("test-key", &signing, SOURCE);
        assert!(parse_and_verify(&contents, |_| Err("unknown".into())).is_err());
    }
}
