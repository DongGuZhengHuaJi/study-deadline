use crate::state::AppState;
use crate::yuketang::{self, FetchError};
use tauri::{AppHandle, Emitter, Manager};

pub fn emit_state_changed(app: &AppHandle) {
    let _ = app.emit("state-changed", ());
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

    let result = yuketang::fetch_snapshot(&session, &account.id).await;
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
