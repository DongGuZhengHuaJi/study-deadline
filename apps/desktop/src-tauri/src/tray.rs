use crate::state::AppState;
use chrono::Utc;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

const TRAY_ID: &str = "main-tray";

pub fn create_tray(app: &AppHandle) -> Result<(), String> {
    let show_hide = MenuItem::with_id(app, "show_hide", "显示 / 隐藏", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let refresh = MenuItem::with_id(app, "refresh", "立即刷新", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|error| error.to_string())?;
    let separator = PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?;
    let menu = Menu::with_items(
        app,
        &[&show_hide, &separator, &refresh, &settings, &quit],
    )
    .map_err(|error| error.to_string())?;

    let Some(icon) = app.default_window_icon().cloned() else {
        return Err("应用图标不可用，无法创建托盘".into());
    };

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("课业倒计时")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show_hide" => toggle_window(app),
            "refresh" => {
                show_window(app);
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = crate::refresh::refresh(&handle).await {
                        tracing::warn!(%error, "manual refresh from tray failed");
                    }
                });
            }
            "settings" => {
                show_window(app);
                let _ = app.emit("ui://open-settings", ());
            }
            "quit" => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.mark_quitting();
                }
                app.exit(0);
            }
            _ => {}
        })
        .build(app)
        .map_err(|error| format!("创建托盘失败: {error}"))?;

    // Tauri drops the tray icon when the last handle is dropped, so retain it
    // for the lifetime of the application.
    app.manage(tray);
    Ok(())
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        match window.is_visible() {
            Ok(true) => {
                let _ = window.hide();
            }
            _ => {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    }
}

pub fn update_tooltip(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let tasks = state.db.load_tasks_all().unwrap_or_default();
    let now = Utc::now().timestamp_millis();
    let pending = tasks
        .iter()
        .filter(|task| task.is_remindable())
        .count();
    let next = tasks
        .iter()
        .filter(|task| task.is_remindable())
        .filter_map(|task| task.target_at())
        .filter(|target| *target > now)
        .min();

    let tooltip = match (pending, next) {
        (0, _) => "课业倒计时 · 暂无待办".to_string(),
        (count, Some(target)) => {
            let minutes = ((target - now) as f64 / 60_000.0).ceil() as i64;
            format!("课业倒计时 · {count} 个待办 · 最近 {minutes} 分钟")
        }
        (count, None) => format!("课业倒计时 · {count} 个待办"),
    };

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}
