use crate::adapters::{self, catalog::UpdateReport, manifest::AdapterInfo, AdapterSmokeResult};
use crate::models::{Dashboard, PlatformInfo, Session, Settings};
use crate::refresh;
use crate::state::{AppState, PendingLogin};
use crate::storage::AccountRow;
use crate::yuketang::{self, USER_AGENT};
use chrono::Utc;
use tauri::{
    webview::WebviewWindowBuilder, AppHandle, LogicalSize, Manager, State, Url, WebviewUrl,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use uuid::Uuid;

pub const LOGIN_LABEL: &str = "login-yuketang";

fn platform_infos(app: &AppHandle) -> Vec<PlatformInfo> {
    adapters::list_all(app)
        .into_iter()
        .map(|adapter| PlatformInfo {
            id: adapter.id,
            name: adapter.name,
            version: adapter.version,
        })
        .collect()
}

#[tauri::command]
pub async fn bootstrap(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Dashboard, String> {
    state.dashboard(platform_infos(&app))
}

#[tauri::command]
pub async fn list_adapters(app: AppHandle) -> Result<Vec<AdapterInfo>, String> {
    Ok(adapters::list_all(&app))
}

#[tauri::command]
pub async fn install_adapter_package(
    app: AppHandle,
    contents: String,
) -> Result<AdapterInfo, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        adapters::install::install_package(&handle, &contents)
    })
    .await
    .map_err(|error| format!("安装适配器线程失败: {error}"))?
}

#[tauri::command]
pub async fn check_adapter_updates(app: AppHandle) -> Result<UpdateReport, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        adapters::catalog::check_and_install(&handle)
    })
    .await
    .map_err(|error| format!("检查适配器更新线程失败: {error}"))?
}

#[tauri::command]
pub async fn run_adapter_smoke(app: AppHandle) -> Result<AdapterSmokeResult, String> {
    let adapter = adapters::load_mock(&app)?;
    let adapter_id = adapter.manifest.id.clone();
    let adapter_version = adapter.manifest.version.clone();
    let snapshot = tauri::async_runtime::spawn_blocking(move || {
        adapters::runtime::run_adapter(&adapter, None, "sdk-smoke")
    })
    .await
    .map_err(|error| format!("适配器线程失败: {error}"))?
    .map_err(|error| error.to_string())?;
    Ok(AdapterSmokeResult {
        adapter_id,
        adapter_version,
        complete: snapshot.complete,
        task_count: snapshot.tasks.len(),
        task_titles: snapshot.tasks.into_iter().map(|task| task.title).collect(),
    })
}

fn is_yuketang_cookie(domain: Option<&str>) -> bool {
    match domain {
        None => true,
        Some(domain) => {
            let domain = domain.trim_start_matches('.');
            domain == "yuketang.cn" || domain.ends_with(".yuketang.cn")
        }
    }
}

fn build_cookie_header(cookies: &[tauri::webview::Cookie<'static>]) -> (String, bool, bool) {
    let relevant: Vec<_> = cookies
        .iter()
        .filter(|cookie| is_yuketang_cookie(cookie.domain()))
        .collect();
    let has_session = relevant.iter().any(|cookie| cookie.name() == "sessionid");
    let has_csrf = relevant.iter().any(|cookie| cookie.name() == "csrftoken");
    let header = relevant
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");
    (header, has_session, has_csrf)
}

#[tauri::command]
pub async fn open_login(
    app: AppHandle,
    state: State<'_, AppState>,
    region_id: String,
    account_id: Option<String>,
) -> Result<(), String> {
    let region = yuketang::region(&region_id)
        .ok_or_else(|| "未知的雨课堂区域".to_string())?;
    if let Some(account_id) = &account_id {
        let account = state
            .account_by_id(account_id)
            .ok_or_else(|| "要重新登录的账号不存在".to_string())?;
        if account.adapter_id != yuketang::ADAPTER_ID {
            return Err("当前登录窗口只支持雨课堂账号".into());
        }
    }
    let url = Url::parse(&format!("{}/v2/web/index", region.base_url))
        .map_err(|error| format!("登录地址无效: {error}"))?;

    if let Some(existing) = app.get_webview_window(LOGIN_LABEL) {
        let _ = existing.destroy();
    }

    WebviewWindowBuilder::new(&app, LOGIN_LABEL, WebviewUrl::External(url))
        .title("雨课堂登录（登录完成后回到主窗口）")
        .inner_size(1120.0, 820.0)
        .min_inner_size(760.0, 560.0)
        .center()
        .always_on_top(true)
        .user_agent(USER_AGENT)
        .on_navigation(|url| {
            let allowed = url.scheme() == "https" || url.as_str() == "about:blank";
            if !allowed {
                tracing::warn!(
                    scheme = url.scheme(),
                    host = url.host_str().unwrap_or("?"),
                    "blocked login navigation"
                );
            }
            allowed
        })
        .build()
        .map_err(|error| format!("创建登录窗口失败: {error}"))?;

    state.set_pending_login(Some(PendingLogin {
        adapter_id: yuketang::ADAPTER_ID.into(),
        region: region.id,
        target_account_id: account_id,
    }));
    Ok(())
}

#[tauri::command]
pub async fn close_login(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LOGIN_LABEL) {
        window
            .destroy()
            .map_err(|error| format!("关闭登录窗口失败: {error}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn finish_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Dashboard, String> {
    let pending = state
        .pending_login()
        .ok_or_else(|| "没有进行中的登录请求".to_string())?;
    if pending.adapter_id != yuketang::ADAPTER_ID {
        return Err("当前登录窗口只支持雨课堂账号".into());
    }
    let region = yuketang::region(&pending.region)
        .ok_or_else(|| "未知的雨课堂区域".to_string())?;
    let window = app
        .get_webview_window(LOGIN_LABEL)
        .ok_or_else(|| "登录窗口不存在，请先打开登录窗口并完成登录".to_string())?;

    let window_for_cookies = window.clone();
    let cookies = tauri::async_runtime::spawn_blocking(move || window_for_cookies.cookies())
        .await
        .map_err(|error| format!("读取 Cookie 任务失败: {error}"))?
        .map_err(|error| format!("读取 Cookie 失败: {error}"))?;
    let (cookie_header, has_session, has_csrf) = build_cookie_header(&cookies);
    if !has_session || cookie_header.is_empty() {
        return Err("没有读取到 sessionid。请确认已在登录窗口完成登录，并保持窗口打开。".into());
    }
    if !has_csrf {
        tracing::warn!("csrftoken missing after login; continuing with GET-only validation");
    }

    let account_id = pending
        .target_account_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let existing = state.account_by_id(&account_id);

    let session = Session {
        base_url: region.base_url.clone(),
        region: region.id.clone(),
        cookie_header,
        csrf_token: cookies
            .iter()
            .find(|cookie| cookie.name() == "csrftoken")
            .map(|cookie| cookie.value().to_string()),
        acquired_at: Utc::now().timestamp_millis(),
    };

    let snapshot = crate::refresh::fetch_account_snapshot(
        &app,
        yuketang::ADAPTER_ID,
        &account_id,
        &session,
    )
    .await
    .map_err(|error| match error {
        yuketang::FetchError::AuthExpired => {
            "会话验证失败：登录已过期或未完成，请重新登录".to_string()
        }
        other => format!("登录后验证失败: {other}"),
    })?;

    let now = Utc::now().timestamp_millis();
    let session_blob = if let Some(key) = state.master_key.as_ref() {
        match crate::secret::encrypt_session(key, &session) {
            Ok(blob) => Some(blob),
            Err(error) => {
                tracing::warn!("{error}; 本次会话不会持久化");
                None
            }
        }
    } else {
        None
    };

    let account = AccountRow {
        id: account_id.clone(),
        adapter_id: yuketang::ADAPTER_ID.into(),
        region: region.id.clone(),
        base_url: region.base_url.clone(),
        display_name: region.label.clone(),
        session_blob,
        created_at: existing
            .as_ref()
            .map(|account| account.created_at)
            .unwrap_or(now),
        last_success_at: Some(now),
        last_error: None,
    };
    state.db.save_account(&account)?;
    state.upsert_account(account, Some(session));
    state.db.replace_tasks(&account_id, &snapshot.tasks, snapshot.complete)?;
    state.mark_account_success(&account_id, now);
    state.finish_refresh(snapshot.warnings);
    state.set_pending_login(None);
    let _ = window.destroy();
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    crate::tray::update_tooltip(&app);
    crate::notifications::check_and_notify(&app);
    refresh::emit_state_changed(&app);
    state.dashboard(platform_infos(&app))
}

#[tauri::command]
pub async fn refresh_now(app: AppHandle) -> Result<(), String> {
    refresh::refresh(&app).await
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Dashboard, String> {
    let next = settings.normalized();
    let previous = state
        .settings
        .read()
        .map_err(|_| "设置状态损坏")?
        .clone();

    if next.autostart != previous.autostart {
        // A dev binary serves its UI from the Vite dev server and must never
        // be registered for login startup. The release/installed build embeds
        // its frontend and is safe for autostart.
        if cfg!(debug_assertions) {
            return Err("开发版不支持开机启动；请使用安装版或发布构建。".into());
        }
        let autolaunch = app.autolaunch();
        if next.autostart {
            autolaunch
                .enable()
                .map_err(|error| format!("开启开机启动失败: {error}"))?;
        } else {
            autolaunch
                .disable()
                .map_err(|error| format!("关闭开机启动失败: {error}"))?;
        }
    }

    state.db.save_settings(&next)?;
    if let Ok(mut current) = state.settings.write() {
        *current = next;
    }

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_always_on_top(
            state
                .settings
                .read()
                .map(|settings| settings.always_on_top)
                .unwrap_or(true),
        );
    }

    refresh::emit_state_changed(&app);
    state.dashboard(platform_infos(&app))
}

#[tauri::command]
pub async fn start_window_drag(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    window
        .start_dragging()
        .map_err(|error| format!("开始拖动窗口失败: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn open_task(
    app: AppHandle,
    state: State<'_, AppState>,
    task_id: String,
) -> Result<(), String> {
    let task = state
        .db
        .task_by_id(&task_id)
        .ok_or_else(|| "任务不存在或已被清理".to_string())?;
    let url = Url::parse(&task.url).map_err(|_| "任务链接无效".to_string())?;
    if url.scheme() != "https" {
        return Err("只允许打开 HTTPS 链接".into());
    }
    let allowed = match adapters::load_by_id(&app, &task.source) {
        Some(adapter) => adapters::runtime::url_allowed_for_adapter(&adapter, &task.url),
        None => {
            let host = url.host_str().unwrap_or_default();
            host == "yuketang.cn" || host.ends_with(".yuketang.cn")
        }
    };
    if !allowed {
        return Err("任务链接域名不在适配器允许范围内".into());
    }

    // Use the user's normal browser. The app's WebView session is deliberately
    // not exported to the system browser; the browser may already be logged in,
    // and if not, the platform will prompt for its own login.
    app.opener()
        .open_url(url.to_string(), None::<&str>)
        .map_err(|error| format!("打开系统浏览器失败: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn logout(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Dashboard, String> {
    let account = state
        .account_by_id(&account_id)
        .ok_or_else(|| "账号不存在".to_string())?;
    state.db.delete_account(&account.id)?;
    state.remove_account(&account.id);
    if state
        .pending_login()
        .and_then(|pending| pending.target_account_id)
        .as_deref()
        == Some(account.id.as_str())
    {
        state.set_pending_login(None);
        if let Some(window) = app.get_webview_window(LOGIN_LABEL) {
            let _ = window.destroy();
        }
    }
    refresh::emit_state_changed(&app);
    state.dashboard(platform_infos(&app))
}

#[tauri::command]
pub async fn hide_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn minimize_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.minimize().map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_view(
    app: AppHandle,
    state: State<'_, AppState>,
    view: String,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "主窗口不存在".to_string())?;
    if view == "settings" {
        window
            .set_size(LogicalSize::new(760.0, 640.0))
            .map_err(|error| error.to_string())?;
        window
            .set_always_on_top(false)
            .map_err(|error| error.to_string())?;
        let _ = window.set_skip_taskbar(false);
    } else {
        window
            .set_size(LogicalSize::new(360.0, 520.0))
            .map_err(|error| error.to_string())?;
        let top = state
            .settings
            .read()
            .map(|settings| settings.always_on_top)
            .unwrap_or(true);
        window
            .set_always_on_top(top)
            .map_err(|error| error.to_string())?;
        let _ = window.set_skip_taskbar(true);
    }
    Ok(())
}

#[tauri::command]
pub async fn quit_app(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.mark_quitting();
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub async fn open_log_dir(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let path = state.log_dir.to_string_lossy().to_string();
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|error| format!("打开日志目录失败: {error}"))?;
    Ok(())
}

#[tauri::command]
pub async fn get_recent_logs(
    state: State<'_, AppState>,
    lines: Option<usize>,
) -> Result<String, String> {
    let path = state.log_dir.join("app.log");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        format!("无法读取日志文件 {}: {error}", path.display())
    });
    let take = lines.unwrap_or(200).clamp(20, 500);
    let tail: Vec<&str> = content.lines().rev().take(take).collect();
    let mut output = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    if output.chars().count() > 80_000 {
        output = output.chars().take(80_000).collect();
    }
    Ok(output)
}
