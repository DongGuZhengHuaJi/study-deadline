use crate::adapters::{self, runtime::AdapterRunError};
use crate::models::{FetchSnapshot, Session};
use crate::state::AppState;
use crate::yuketang::{self, FetchError};
use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};

pub fn emit_state_changed(app: &AppHandle) {
    let _ = app.emit("state-changed", ());
}

pub async fn fetch_account_snapshot(
    app: &AppHandle,
    adapter_id: &str,
    account_id: &str,
    session: &Session,
) -> Result<FetchSnapshot, FetchError> {
    if let Some(adapter) = adapters::load_by_id(app, adapter_id) {
        let version = adapter.manifest.version.clone();
        tracing::info!(
            adapter = %adapter_id,
            version = %version,
            account = %account_id,
            "using SDK adapter runtime"
        );
        let session = session.clone();
        let account_id = account_id.to_string();
        let result = tauri::async_runtime::spawn_blocking(move || {
            adapters::runtime::run_adapter(&adapter, Some(&session), &account_id)
        })
        .await
        .map_err(|error| FetchError::Network(format!("适配器线程失败: {error}")))?;
        return match result {
            Ok(snapshot) => Ok(snapshot),
            Err(AdapterRunError::AuthExpired) => Err(FetchError::AuthExpired),
            Err(AdapterRunError::RateLimited) => {
                Err(FetchError::RateLimited("SDK adapter rate limited".into()))
            }
            Err(AdapterRunError::SchemaChanged(message)) => Err(FetchError::Schema(message)),
            Err(error) => Err(FetchError::Schema(error.to_string())),
        };
    }
    yuketang::fetch_snapshot(session, account_id).await
}

/// Refresh every account that has a session. Accounts are independent: one
/// expired session does not stop the others.
pub async fn refresh(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state.refresh_lock.lock().await;

    let accounts = state.accounts_snapshot()?;
    if accounts.is_empty() {
        return Err("尚未添加学习平台账号".into());
    }
    let sessions = state.sessions_snapshot()?;
    if sessions.is_empty() {
        return Err("没有可用的登录会话，请先登录".into());
    }

    state.begin_refresh();
    emit_state_changed(app);

    let mut warnings = Vec::new();
    let mut successes = 0usize;
    let mut errors = Vec::new();

    for account in accounts {
        let Some(session) = sessions.get(&account.id).cloned() else {
            continue;
        };
        tracing::info!(
            adapter = %account.adapter_id,
            account = %account.id,
            "refresh started"
        );
        let result =
            fetch_account_snapshot(app, &account.adapter_id, &account.id, &session).await;
        match result {
            Ok(snapshot) => {
                state
                    .db
                    .replace_tasks(&account.id, &snapshot.tasks, snapshot.complete)?;
                let now = Utc::now().timestamp_millis();
                state.mark_account_success(&account.id, now);
                successes += 1;
                warnings.extend(snapshot.warnings);
                tracing::info!(
                    account = %account.id,
                    tasks = snapshot.tasks.len(),
                    complete = snapshot.complete,
                    "refresh succeeded"
                );
            }
            Err(FetchError::AuthExpired) => {
                state.clear_account_session(&account.id);
                state.mark_account_error(&account.id, "登录已失效，请重新登录");
                errors.push(format!("{}: 登录已失效", account.display_name));
                tracing::warn!(account = %account.id, "session expired");
            }
            Err(error) => {
                let message = error.to_string();
                state.mark_account_error(&account.id, &message);
                errors.push(format!("{}: {message}", account.display_name));
                tracing::warn!(account = %account.id, %message, "refresh failed");
            }
        }
    }

    state.finish_refresh(warnings);
    crate::tray::update_tooltip(app);
    crate::notifications::check_and_notify(app);
    emit_state_changed(app);

    if successes == 0 {
        Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| "所有账号刷新失败".into()))
    } else {
        Ok(())
    }
}
