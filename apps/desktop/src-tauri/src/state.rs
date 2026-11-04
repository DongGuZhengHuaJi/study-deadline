use crate::models::{
    AccountView, Dashboard, PlatformInfo, Session, Settings, SyncView,
};
use crate::platform;
use crate::secret::{decrypt_session, MasterKey};
use crate::storage::{AccountRow, Database};
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

#[derive(Clone, Debug)]
pub struct PendingLogin {
    pub adapter_id: String,
    pub region: String,
    pub target_account_id: Option<String>,
}

pub struct AppState {
    pub db: Database,
    pub settings: RwLock<Settings>,
    pub accounts: RwLock<Vec<AccountRow>>,
    pub sessions: RwLock<HashMap<String, Session>>,
    pub sync: RwLock<SyncView>,
    pub pending_login: RwLock<Option<PendingLogin>>,
    pub master_key: Option<MasterKey>,
    pub log_dir: PathBuf,
    pub refresh_lock: tokio::sync::Mutex<()>,
    pub wake: Notify,
    pub quitting: AtomicBool,
}

impl AppState {
    pub fn initialize(app: &AppHandle, log_dir: PathBuf) -> Result<Self, String> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("无法定位应用数据目录: {error}"))?;
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| format!("创建应用数据目录失败: {error}"))?;

        let db = Database::open(&data_dir.join("study-deadline.sqlite3"))?;
        let mut settings = db.load_settings();
        // Never leave a dev build registered as a login-startup application.
        // A dev binary depends on the Vite server and would show a blank
        // localhost page after reboot.
        if cfg!(debug_assertions) && settings.autostart {
            settings.autostart = false;
            let _ = db.save_settings(&settings);
        }
        let accounts = db.load_accounts()?;

        let master_key = match crate::secret::load_or_create_master_key() {
            Ok(key) => Some(key),
            Err(error) => {
                tracing::warn!("{error}; 会话将仅在本次运行内有效");
                None
            }
        };

        let mut sessions = HashMap::new();
        if let Some(key) = master_key.as_ref() {
            for account in &accounts {
                let Some(encrypted) = account.session_blob.as_deref() else {
                    continue;
                };
                match decrypt_session(key, encrypted) {
                    Ok(session) => {
                        sessions.insert(account.id.clone(), session);
                    }
                    Err(error) => {
                        tracing::warn!(account = %account.id, "无法解密保存的会话: {error}");
                    }
                }
            }
        }

        let has_accounts = !accounts.is_empty();
        let sync = SyncView {
            refreshing: false,
            last_attempt_at: None,
            last_success_at: accounts.iter().filter_map(|account| account.last_success_at).max(),
            last_error: accounts.iter().find_map(|account| account.last_error.clone()),
            needs_login: has_accounts && accounts.iter().any(|account| !sessions.contains_key(&account.id)),
            warnings: Vec::new(),
        };

        Ok(Self {
            db,
            settings: RwLock::new(settings),
            accounts: RwLock::new(accounts),
            sessions: RwLock::new(sessions),
            sync: RwLock::new(sync),
            pending_login: RwLock::new(None),
            master_key,
            log_dir,
            refresh_lock: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            quitting: AtomicBool::new(false),
        })
    }

    pub fn dashboard(&self, platforms: Vec<PlatformInfo>) -> Result<Dashboard, String> {
        let accounts = self.accounts.read().map_err(|_| "账号状态损坏")?.clone();
        let sessions = self.sessions.read().map_err(|_| "会话状态损坏")?;
        let tasks = self.db.load_tasks_all()?;
        let settings = self.settings.read().map_err(|_| "设置状态损坏")?.clone();
        let mut sync = self.sync.read().map_err(|_| "同步状态损坏")?.clone();
        sync.needs_login = accounts.iter().any(|account| !sessions.contains_key(&account.id));

        let account_views = accounts
            .iter()
            .map(|account| AccountView {
                id: account.id.clone(),
                adapter_id: account.adapter_id.clone(),
                display_name: account.display_name.clone(),
                region: account.region.clone(),
                base_url: account.base_url.clone(),
                authenticated: sessions.contains_key(&account.id),
                needs_login: !sessions.contains_key(&account.id),
                refreshing: sync.refreshing,
                last_success_at: account.last_success_at,
                last_error: account.last_error.clone(),
            })
            .collect();

        Ok(Dashboard {
            tasks,
            accounts: account_views,
            platforms,
            settings,
            sync,
            regions: platform::yuketang_regions(),
            log_path: self.log_dir.join("app.log").to_string_lossy().to_string(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    pub fn begin_refresh(&self) {
        if let Ok(mut sync) = self.sync.write() {
            sync.refreshing = true;
            sync.last_attempt_at = Some(Utc::now().timestamp_millis());
        }
    }

    pub fn finish_refresh(&self, warnings: Vec<String>) {
        let now = Utc::now().timestamp_millis();
        let accounts = self
            .accounts
            .read()
            .map(|accounts| accounts.clone())
            .unwrap_or_default();
        let session_ids: std::collections::HashSet<String> = self
            .sessions
            .read()
            .map(|sessions| sessions.keys().cloned().collect())
            .unwrap_or_default();
        if let Ok(mut sync) = self.sync.write() {
            sync.refreshing = false;
            sync.last_attempt_at = Some(now);
            sync.last_success_at = accounts.iter().filter_map(|account| account.last_success_at).max();
            sync.last_error = accounts.iter().find_map(|account| account.last_error.clone());
            sync.needs_login = !accounts.is_empty()
                && accounts.iter().any(|account| !session_ids.contains(&account.id));
            sync.warnings = warnings;
        }
    }

    pub fn accounts_snapshot(&self) -> Result<Vec<AccountRow>, String> {
        self.accounts
            .read()
            .map(|accounts| accounts.clone())
            .map_err(|_| "账号状态损坏".into())
    }

    pub fn account_by_id(&self, account_id: &str) -> Option<AccountRow> {
        self.accounts
            .read()
            .ok()?
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
    }

    pub fn sessions_snapshot(&self) -> Result<HashMap<String, Session>, String> {
        self.sessions
            .read()
            .map(|sessions| sessions.clone())
            .map_err(|_| "会话状态损坏".into())
    }

    pub fn upsert_account(&self, account: AccountRow, session: Option<Session>) {
        let account_id = account.id.clone();
        if let Ok(mut accounts) = self.accounts.write() {
            if let Some(existing) = accounts.iter_mut().find(|existing| existing.id == account_id) {
                *existing = account;
            } else {
                accounts.push(account);
            }
        }
        if let Some(session) = session {
            if let Ok(mut sessions) = self.sessions.write() {
                sessions.insert(account_id, session);
            }
        }
    }

    pub fn remove_account(&self, account_id: &str) {
        if let Ok(mut accounts) = self.accounts.write() {
            accounts.retain(|account| account.id != account_id);
        }
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.remove(account_id);
        }
        self.finish_refresh(Vec::new());
    }

    pub fn mark_account_success(&self, account_id: &str, at: i64) {
        let _ = self.db.mark_success(account_id, at);
        if let Ok(mut accounts) = self.accounts.write() {
            if let Some(account) = accounts.iter_mut().find(|account| account.id == account_id) {
                account.last_success_at = Some(at);
                account.last_error = None;
            }
        }
    }

    pub fn mark_account_error(&self, account_id: &str, message: &str) {
        let _ = self.db.mark_error(account_id, message);
        if let Ok(mut accounts) = self.accounts.write() {
            if let Some(account) = accounts.iter_mut().find(|account| account.id == account_id) {
                account.last_error = Some(message.to_string());
            }
        }
    }

    pub fn clear_account_session(&self, account_id: &str) {
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.remove(account_id);
        }
        let _ = self.db.update_session_blob(account_id, None);
    }

    pub fn set_pending_login(&self, pending: Option<PendingLogin>) {
        if let Ok(mut current) = self.pending_login.write() {
            *current = pending;
        }
    }

    pub fn pending_login(&self) -> Option<PendingLogin> {
        self.pending_login.read().ok().and_then(|pending| pending.clone())
    }

    pub fn should_quit(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }
}
