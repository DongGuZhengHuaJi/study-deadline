use crate::models::{
    AccountView, Dashboard, Session, Settings, SyncView,
};
use crate::secret::{decrypt_session, MasterKey};
use crate::storage::{AccountRow, Database};
use crate::yuketang;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

pub struct AppState {
    pub db: Database,
    pub settings: RwLock<Settings>,
    pub session: RwLock<Option<Session>>,
    pub account: RwLock<Option<AccountRow>>,
    pub sync: RwLock<SyncView>,
    pub login_region: RwLock<Option<String>>,
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
        let account = db.load_account(yuketang::ADAPTER_ID);

        let master_key = match crate::secret::load_or_create_master_key() {
            Ok(key) => Some(key),
            Err(error) => {
                tracing::warn!("{error}; 会话将仅在本次运行内有效");
                None
            }
        };

        let session = account.as_ref().and_then(|account| {
            let encrypted = account.session_blob.as_deref()?;
            let key = master_key.as_ref()?;
            match decrypt_session(key, encrypted) {
                Ok(session) => Some(session),
                Err(error) => {
                    tracing::warn!("无法解密保存的会话: {error}");
                    None
                }
            }
        });

        let sync = SyncView {
            refreshing: false,
            last_attempt_at: None,
            last_success_at: account.as_ref().and_then(|account| account.last_success_at),
            last_error: account.as_ref().and_then(|account| account.last_error.clone()),
            needs_login: session.is_none() && account.is_some(),
            warnings: Vec::new(),
        };

        Ok(Self {
            db,
            settings: RwLock::new(settings),
            session: RwLock::new(session),
            account: RwLock::new(account),
            sync: RwLock::new(sync),
            login_region: RwLock::new(None),
            master_key,
            log_dir,
            refresh_lock: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
            quitting: AtomicBool::new(false),
        })
    }

    pub fn dashboard(&self) -> Result<Dashboard, String> {
        let account = self.account.read().map_err(|_| "账号状态损坏")?.clone();
        let tasks = match &account {
            Some(account) => self.db.load_tasks(&account.id)?,
            None => Vec::new(),
        };
        let settings = self
            .settings
            .read()
            .map_err(|_| "设置状态损坏")?
            .clone();
        let sync = self.sync.read().map_err(|_| "同步状态损坏")?.clone();
        let authenticated = self
            .session
            .read()
            .map_err(|_| "会话状态损坏")?
            .is_some();

        Ok(Dashboard {
            tasks,
            account: account.map(|account| AccountView {
                id: account.id,
                display_name: account.display_name,
                region: account.region,
                base_url: account.base_url,
                authenticated,
                last_success_at: account.last_success_at,
                last_error: account.last_error,
            }),
            settings,
            sync,
            regions: yuketang::regions(),
            log_path: self.log_dir.join("app.log").to_string_lossy().to_string(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    pub fn set_sync_refreshing(&self, refreshing: bool) {
        if let Ok(mut sync) = self.sync.write() {
            sync.refreshing = refreshing;
            sync.last_attempt_at = Some(Utc::now().timestamp_millis());
        }
    }

    pub fn record_sync_success(&self, warnings: Vec<String>) {
        let now = Utc::now().timestamp_millis();
        if let Ok(mut sync) = self.sync.write() {
            sync.refreshing = false;
            sync.last_attempt_at = Some(now);
            sync.last_success_at = Some(now);
            sync.last_error = None;
            sync.needs_login = false;
            sync.warnings = warnings;
        }
        let account_id = self
            .account
            .read()
            .ok()
            .and_then(|account| account.as_ref().map(|account| account.id.clone()));
        if let Some(account_id) = account_id {
            let _ = self.db.mark_success(&account_id, now);
            if let Ok(mut account) = self.account.write() {
                if let Some(account) = account.as_mut() {
                    account.last_success_at = Some(now);
                    account.last_error = None;
                }
            }
        }
    }

    pub fn record_sync_error(&self, message: &str, needs_login: bool) {
        if let Ok(mut sync) = self.sync.write() {
            sync.refreshing = false;
            sync.last_attempt_at = Some(Utc::now().timestamp_millis());
            sync.last_error = Some(message.to_string());
            sync.needs_login = needs_login;
        }
        let account_id = self
            .account
            .read()
            .ok()
            .and_then(|account| account.as_ref().map(|account| account.id.clone()));
        if let Some(account_id) = account_id {
            let _ = self.db.mark_error(&account_id, message);
            if let Ok(mut account) = self.account.write() {
                if let Some(account) = account.as_mut() {
                    account.last_error = Some(message.to_string());
                }
            }
        }
    }

    pub fn set_session(&self, session: Option<Session>) {
        if let Ok(mut current) = self.session.write() {
            *current = session;
        }
    }

    pub fn set_account(&self, account: Option<AccountRow>) {
        if let Ok(mut current) = self.account.write() {
            *current = account;
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quitting.load(Ordering::SeqCst)
    }

    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }
}
