use crate::models::Session;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

const KEYRING_SERVICE: &str = "dev.donggu.studydeadline";
const KEYRING_USER: &str = "session-master-key-v1";

pub type MasterKey = [u8; 32];

/// Load the per-installation master key from the OS credential store.
///
/// If the credential store is unavailable, callers should keep the session in
/// memory only. We intentionally do not fall back to a plaintext key file.
pub fn load_or_create_master_key() -> Result<MasterKey, String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|error| format!("OS 凭据库不可用: {error}"))?;

    match entry.get_password() {
        Ok(encoded) => {
            let bytes = STANDARD
                .decode(encoded.trim())
                .map_err(|error| format!("保存的会话主密钥无效: {error}"))?;
            if bytes.len() != 32 {
                return Err("保存的会话主密钥长度错误".into());
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(key)
        }
        Err(_) => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key).map_err(|error| format!("生成主密钥失败: {error}"))?;
            entry
                .set_password(&STANDARD.encode(key))
                .map_err(|error| format!("写入 OS 凭据库失败: {error}"))?;
            Ok(key)
        }
    }
}

pub fn encrypt_session(key: &MasterKey, session: &Session) -> Result<Vec<u8>, String> {
    let plaintext =
        serde_json::to_vec(session).map_err(|error| format!("序列化会话失败: {error}"))?;
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "创建加密器失败".to_string())?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).map_err(|error| format!("生成随机数失败: {error}"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_slice())
        .map_err(|_| "加密会话失败".to_string())?;
    let mut output = Vec::with_capacity(12 + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

pub fn decrypt_session(key: &MasterKey, encrypted: &[u8]) -> Result<Session, String> {
    if encrypted.len() < 13 {
        return Err("会话密文长度错误".into());
    }
    let (nonce_bytes, ciphertext) = encrypted.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "创建解密器失败".to_string())?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| "解密会话失败（凭据库密钥可能已变化）".to_string())?;
    serde_json::from_slice(&plaintext).map_err(|error| format!("解析会话失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            base_url: "https://www.yuketang.cn".into(),
            region: "www".into(),
            cookie_header: "sessionid=test; csrftoken=abc".into(),
            csrf_token: Some("abc".into()),
            acquired_at: 1,
        }
    }

    #[test]
    fn round_trip_session() {
        let key = [7u8; 32];
        let encrypted = encrypt_session(&key, &session()).unwrap();
        let decrypted = decrypt_session(&key, &encrypted).unwrap();
        assert_eq!(decrypted.cookie_header, session().cookie_header);
        assert_eq!(decrypted.csrf_token, Some("abc".into()));
    }

    #[test]
    fn wrong_key_fails() {
        let encrypted = encrypt_session(&[1u8; 32], &session()).unwrap();
        assert!(decrypt_session(&[2u8; 32], &encrypted).is_err());
    }
}
