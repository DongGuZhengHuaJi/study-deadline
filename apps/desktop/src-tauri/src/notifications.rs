use crate::models::Task;
use crate::state::AppState;
use chrono::Utc;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

fn humanize(minutes: u64) -> String {
    if minutes >= 1440 {
        let days = minutes / 1440;
        let hours = (minutes % 1440) / 60;
        if hours > 0 {
            format!("{days} 天 {hours} 小时")
        } else {
            format!("{days} 天")
        }
    } else if minutes >= 60 {
        let hours = minutes / 60;
        let rest = minutes % 60;
        if rest > 0 {
            format!("{hours} 小时 {rest} 分钟")
        } else {
            format!("{hours} 小时")
        }
    } else {
        format!("{minutes} 分钟")
    }
}

fn notification_copy(task: &Task, remaining_minutes: u64) -> (String, String) {
    let title = match task.kind.as_str() {
        "exam" => "考试即将开始",
        "quiz" => "测验即将截止",
        _ => "作业即将截止",
    };
    let body = format!(
        "{}\n{} · 还剩 {}",
        task.title,
        task.course,
        humanize(remaining_minutes)
    );
    (title.to_string(), body)
}

/// Check all cached tasks and emit at most one notification per task per
/// threshold window. Called after refreshes and once per minute by the
/// scheduler.
pub fn check_and_notify(app: &AppHandle) {
    let state = app.state::<AppState>();
    let settings = match state.settings.read() {
        Ok(settings) => settings.clone(),
        Err(_) => return,
    };
    if !settings.notifications_enabled {
        return;
    }
    let Some(account) = state
        .account
        .read()
        .ok()
        .and_then(|account| account.clone())
    else {
        return;
    };
    let tasks = match state.db.load_tasks(&account.id) {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, "load tasks for notifications failed");
            return;
        }
    };

    let now = Utc::now().timestamp_millis();
    let mut thresholds: Vec<u64> = settings.notify_lead_minutes.clone();
    thresholds.sort_unstable();

    for task in tasks {
        if !task.is_remindable() {
            continue;
        }
        let Some(target) = task.target_at() else {
            continue;
        };
        let remaining_ms = target.saturating_sub(now);
        if remaining_ms <= 0 {
            continue;
        }
        let remaining_minutes = ((remaining_ms as f64) / 60_000.0).ceil() as u64;
        let Some(threshold) = thresholds
            .iter()
            .copied()
            .find(|threshold| *threshold >= remaining_minutes)
        else {
            continue;
        };
        if state.db.was_notified(&task.id, threshold) {
            continue;
        }

        let (title, body) = notification_copy(&task, remaining_minutes);
        match app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            Ok(()) => {
                // Mark this and every larger lead time as sent so a late
                // startup does not produce a burst of catch-up notifications.
                for lead in thresholds.iter().copied().filter(|lead| *lead >= threshold) {
                    let _ = state.db.mark_notified(&task.id, lead, now);
                }
                tracing::info!(
                    task = %task.remote_id,
                    threshold_minutes = threshold,
                    "notification sent"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "notification failed");
            }
        }
    }
}
