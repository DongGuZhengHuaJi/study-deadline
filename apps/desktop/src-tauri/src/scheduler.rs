use crate::refresh;
use crate::state::AppState;
use chrono::Utc;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// One background loop owns both timed refreshes and minute-level reminder
/// checks. The wall clock is used so suspend/resume does not stall scheduling.
pub async fn run(app: AppHandle) {
    // Give the window a moment to appear before the first network refresh.
    tokio::time::sleep(Duration::from_secs(3)).await;

    loop {
        let state = app.state::<AppState>();
        let settings = state
            .settings
            .read()
            .map(|settings| settings.clone())
            .unwrap_or_default();
        let has_session = state
            .sessions
            .read()
            .map(|sessions| !sessions.is_empty())
            .unwrap_or(false);

        if has_session {
            let last_attempt = state
                .sync
                .read()
                .ok()
                .and_then(|sync| sync.last_attempt_at)
                .unwrap_or(0);
            let now = Utc::now().timestamp_millis();
            let interval_ms = settings.refresh_minutes as i64 * 60_000;
            if now.saturating_sub(last_attempt) >= interval_ms {
                if let Err(error) = refresh::refresh(&app).await {
                    tracing::warn!(%error, "scheduled refresh failed");
                }
            }
        }

        crate::notifications::check_and_notify(&app);
        crate::tray::update_tooltip(&app);

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {},
            _ = state.wake.notified() => {},
        }
    }
}
