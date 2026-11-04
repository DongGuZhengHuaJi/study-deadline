mod adapters;
mod commands;
mod models;
mod notifications;
mod platform;
mod refresh;
mod scheduler;
mod secret;
mod state;
mod storage;
mod tray;

use state::AppState;
use std::path::PathBuf;
use tauri::{Manager, WindowEvent};
use tracing_subscriber::EnvFilter;

fn init_logging(log_dir: &PathBuf) -> Result<(), String> {
    std::fs::create_dir_all(log_dir)
        .map_err(|error| format!("创建日志目录失败: {error}"))?;
    let appender = tracing_appender::rolling::never(log_dir, "app.log");
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,study_deadline=debug"));
    if let Err(error) = tracing_subscriber::fmt()
        .with_writer(appender)
        .with_ansi(false)
        .with_target(true)
        .with_env_filter(filter)
        .try_init()
    {
        // Logging must never make the tray-only application unstartable.
        eprintln!("初始化日志失败: {error}");
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be registered before any other plugin so a second instance can
        // hand control back to the existing window.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let log_dir = app
                .path()
                .app_log_dir()
                .map_err(|error| format!("无法定位日志目录: {error}"))?;
            init_logging(&log_dir)?;
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "application starting");

            let state = AppState::initialize(&app.handle(), log_dir)?;
            let should_refresh = state
                .sessions
                .read()
                .map(|sessions| !sessions.is_empty())
                .unwrap_or(false);
            app.manage(state);

            if let Err(error) = tray::create_tray(&app.handle()) {
                tracing::warn!(%error, "tray creation failed");
                // Do not let a failed tray make the frameless window
                // unreachable; leave it in the taskbar as a fallback.
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_skip_taskbar(false);
                }
            }

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                scheduler::run(handle).await;
            });

            if should_refresh {
                tracing::info!("existing session found; scheduler will refresh");
            } else {
                tracing::info!("no valid session; waiting for user login");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let quitting = window
                    .app_handle()
                    .try_state::<AppState>()
                    .map(|state| state.should_quit())
                    .unwrap_or(false);
                if !quitting {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::list_adapters,
            commands::install_adapter_package,
            commands::check_adapter_updates,
            commands::run_adapter_smoke,
            commands::open_login,
            commands::close_login,
            commands::finish_login,
            commands::refresh_now,
            commands::update_settings,
            commands::open_task,
            commands::start_window_drag,
            commands::logout,
            commands::hide_main_window,
            commands::minimize_main_window,
            commands::set_view,
            commands::quit_app,
            commands::open_log_dir,
            commands::get_recent_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
