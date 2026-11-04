use crate::adapters::{self, runtime::AdapterRunError};
use crate::models::{FetchSnapshot, Session};
use crate::state::AppState;
use crate::yuketang::{self, FetchError};
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

/// Fetch a new snapshot for the current account.
///
/// The refresh lock serialises scheduled and manual refreshes. Old tasks are
/// only replaced after the corresponding database write succeeds.
pub async fn refresh(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state.refresh_lock.lock().await;

    let account = state
        .account
        .read()
        .map_err(|_| "账号状态损坏")?
        .clone()
        .ok_or_else(|| "尚未登录雨课堂".to_string())?;
    let session = state
        .session
        .read()
        .map_err(|_| "会话状态损坏")?
        .clone()
        .ok_or_else(|| "会话不存在，请重新登录".to_string())?;

    state.set_sync_refreshing(true);
    emit_state_changed(app);

    tracing::info!(
        adapter = yuketang::ADAPTER_ID,
        account = %account.id,
        "refresh started"
    );

    let result = fetch_account_snapshot(app, &account.adapter_id, &account.id, &session).await;
    match result {
        Ok(snapshot) => {
            state
                .db
                .replace_tasks(&account.id, &snapshot.tasks, snapshot.complete)?;
            if !snapshot.complete {
                tracing::warn!(
                    warnings = snapshot.warnings.len(),
                    "refresh completed with warnings"
                );
            }
            state.record_sync_success(snapshot.warnings);
            tracing::info!(
                tasks = snapshot.tasks.len(),
                courses = snapshot.course_count,
                complete = snapshot.complete,
                "refresh succeeded"
            );
            crate::tray::update_tooltip(app);
            crate::notifications::check_and_notify(app);
            emit_state_changed(app);
            Ok(())
        }
        Err(FetchError::AuthExpired) => {
            state.set_session(None);
            let _ = state.db.update_session_blob(&account.id, None);
            state.record_sync_error("登录已失效，请重新登录", true);
            tracing::warn!("session expired");
            emit_state_changed(app);
            Err("登录已失效，请重新登录".into())
        }
        Err(error) => {
            let message = error.to_string();
            state.record_sync_error(&message, false);
            tracing::warn!(%message, "refresh failed");
            emit_state_changed(app);
            Err(message)
        }
    }
}
