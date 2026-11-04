use crate::adapters::{
    self,
    catalog::UpdateReport,
    manifest::{cookie_domain_allowed, AdapterInfo},
    AdapterSmokeResult,
};
use crate::models::{Dashboard, PlatformInfo, RegionInfo, Session, Settings};
use crate::platform::{self, USER_AGENT};
use crate::refresh;
use crate::state::{AppState, PendingLogin};
use crate::storage::AccountRow;
use chrono::Utc;
use std::collections::HashSet;
use tauri::{
    webview::WebviewWindowBuilder, AppHandle, LogicalSize, Manager, State, Url, WebviewUrl,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;
use uuid::Uuid;

pub const LOGIN_LABEL: &str = "login";

/// Platform list for the UI. Adapter packages may include the same id in both
/// the built-in directory and the installed directory; only the best
/// compatible package is shown.
fn platform_infos(app: &AppHandle) -> Vec<PlatformInfo> {
    let mut seen = HashSet::new();
    let mut platforms = Vec::new();
    for info in adapters::list_all(app) {
        if info.id == adapters::MOCK_ADAPTER_ID || !seen.insert(info.id.clone()) {
            continue;
        }
        let Some(adapter) = adapters::load_by_id(app, &info.id) else {
            continue;
        };
        let login_url = adapter
            .manifest
            .auth
            .as_ref()
            .map(|auth| auth.login_url.clone());
        let regions = if info.id == platform::YUKETANG_ADAPTER_ID {
            platform::yuketang_regions()
        } else if let Some(auth) = adapter.manifest.auth.as_ref() {
            vec![RegionInfo {
                id: "default".into(),
                label: adapter.manifest.name.clone(),
                base_url: auth.resolved_base_url().unwrap_or_default(),
            }]
        } else {
            Vec::new()
        };
        platforms.push(PlatformInfo {
            id: info.id,
            name: adapter.manifest.name,
            version: adapter.manifest.version,
            login_url,
            regions,
        });
    }
    platforms.sort_by(|left, right| left.id.cmp(&right.id));
    platforms
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

#[tauri::command]
pub async fn open_login(
    app: AppHandle,
    state: State<'_, AppState>,
    adapter_id: Option<String>,
    region_id: Option<String>,
    account_id: Option<String>,
) -> Result<(), String> {
    let adapter_id =
        adapter_id.unwrap_or_else(|| platform::YUKETANG_ADAPTER_ID.into());
    if let Some(account_id) = &account_id {
        let account = state
            .account_by_id(account_id)
            .ok_or_else(|| "要重新登录的账号不存在".to_string())?;
        if account.adapter_id != adapter_id {
            return Err("账号所属平台与登录入口不一致".into());
        }
    }
    let adapter = adapters::load_by_id(&app, &adapter_id)
        .ok_or_else(|| format!("找不到平台适配器: {adapter_id}"))?;
    let auth = adapter
        .manifest
        .auth
        .as_ref()
        .ok_or_else(|| format!("{} 适配器未声明登录入口", adapter.manifest.name))?;

    let (region_id, login_url) = if adapter_id == platform::YUKETANG_ADAPTER_ID {
        let region = platform::yuketang_region(region_id.as_deref().unwrap_or("www"))
            .ok_or_else(|| "未知的雨课堂区域".to_string())?;
        (
            region.id,
            format!("{}/v2/web/index", region.base_url.trim_end_matches('/')),
        )
    } else {
        (
            region_id.unwrap_or_else(|| "default".into()),
            auth.login_url.clone(),
        )
    };
    let url = Url::parse(&login_url).map_err(|error| format!("登录地址无效: {error}"))?;
    if url.scheme() != "https" {
        return Err("登录地址必须是 HTTPS".into());
    }
    let navigation_domains = auth.cookie_domains(&adapter.manifest.permissions.http);

    if let Some(existing) = app.get_webview_window(LOGIN_LABEL) {
        let _ = existing.destroy();
    }
    tracing::info!(
        adapter = %adapter_id,
        host = url.host_str().unwrap_or("?"),
        path = url.path(),
        "opening login window"
    );

    // Use an ephemeral WebView profile for login. Persisting or sharing the
    // platform cookie jar between login attempts can leave a stale routing /
    // session cookie that makes Chaoxing bounce between passport2 and the
    // platform in a redirect loop ("cannot follow more than 20 redirections").
    // The session is harvested into the encrypted app store afterwards, so the
    // WebView itself does not need persistent cookies.
    let window = WebviewWindowBuilder::new(&app, LOGIN_LABEL, WebviewUrl::External(url))
        .title(format!(
            "{}登录（登录完成后回到主窗口）",
            adapter.manifest.name
        ))
        .inner_size(1120.0, 820.0)
        .min_inner_size(760.0, 560.0)
        .center()
        .always_on_top(true)
        .incognito(true)
        .user_agent(USER_AGENT)
        .on_navigation(move |url| {
            // School SSO/CAS flows leave the platform domain, so the login
            // window must allow ordinary HTTPS navigation. Plain HTTP is only
            // allowed for the platform's own domains as a redirect fallback;
            // it is never accepted by the adapter's API host.
            let scheme = url.scheme();
            let platform_http = scheme == "http"
                && url
                    .host_str()
                    .map(|host| cookie_domain_allowed(Some(host), &navigation_domains))
                    .unwrap_or(false);
            let allowed = scheme == "https"
                || url.as_str() == "about:blank"
                || platform_http;
            tracing::debug!(
                scheme = %scheme,
                host = url.host_str().unwrap_or("?"),
                path = url.path(),
                allowed,
                "login navigation"
            );
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

    // WebKitGTK defaults to ACCEPT_NO_THIRD_PARTY, but Chaoxing's
    // passport2 -> SSO -> i.chaoxing.com redirect chain needs cookies set on
    // chaoxing.com from a different host. With the default policy the login is
    // thrown back to passport2 repeatedly and WebKit eventually fails with
    // "cannot follow more than 20 redirections". The login WebView is
    // ephemeral, so accepting all cookies here does not affect the app store.
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::{CookieManagerExt, WebViewExt, WebsiteDataManagerExt};
        window
            .with_webview(|webview| {
                if let Some(manager) = webview
                    .inner()
                    .website_data_manager()
                    .and_then(|data| data.cookie_manager())
                {
                    manager.set_accept_policy(webkit2gtk::CookieAcceptPolicy::Always);
                }
            })
            .map_err(|error| format!("设置登录 Cookie 策略失败: {error}"))?;
    }

    state.set_pending_login(Some(PendingLogin {
        adapter_id,
        region: region_id,
        target_account_id: account_id,
    }));
    Ok(())
}

#[tauri::command]
pub async fn close_login(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(LOGIN_LABEL) {
        window
            .destroy()
            .map_err(|error| format!("关闭登录窗口失败: {error}"))?;
    }
    state.set_pending_login(None);
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
    let adapter = adapters::load_by_id(&app, &pending.adapter_id)
        .ok_or_else(|| format!("找不到平台适配器: {}", pending.adapter_id))?;
    let auth = adapter
        .manifest
        .auth
        .as_ref()
        .ok_or_else(|| format!("{} 适配器未声明登录", adapter.manifest.name))?;
    let (region_id, base_url, default_display_name) =
        if pending.adapter_id == platform::YUKETANG_ADAPTER_ID {
            let region = platform::yuketang_region(&pending.region)
                .ok_or_else(|| "未知的雨课堂区域".to_string())?;
            (region.id, region.base_url, region.label)
        } else {
            (
                pending.region.clone(),
                auth.resolved_base_url()?,
                adapter.manifest.name.clone(),
            )
        };
    let window = app
        .get_webview_window(LOGIN_LABEL)
        .ok_or_else(|| "登录窗口不存在，请先打开登录窗口并完成登录".to_string())?;

    let window_for_cookies = window.clone();
    let cookies = tauri::async_runtime::spawn_blocking(move || window_for_cookies.cookies())
        .await
        .map_err(|error| format!("读取 Cookie 任务失败: {error}"))?
        .map_err(|error| format!("读取 Cookie 失败: {error}"))?;
    let cookie_domains = auth.cookie_domains(&adapter.manifest.permissions.http);
    let relevant: Vec<_> = cookies
        .iter()
        .filter(|cookie| match cookie.domain() {
            Some(domain) => cookie_domain_allowed(Some(domain), &cookie_domains),
            // Host-only cookies cannot be attributed to a domain; accept only
            // explicitly declared session cookie names.
            None => auth
                .session_cookies
                .iter()
                .any(|name| cookie.name() == name),
        })
        .collect();
    let required = if auth.required_any_cookies.is_empty() {
        &auth.session_cookies
    } else {
        &auth.required_any_cookies
    };
    tracing::info!(
        adapter = %pending.adapter_id,
        cookies = relevant.len(),
        "login cookies extracted"
    );
    let has_required = required.is_empty()
        || relevant
            .iter()
            .any(|cookie| required.iter().any(|name| cookie.name() == name));
    if !has_required || relevant.is_empty() {
        return Err(format!(
            "没有读取到 {} 的有效登录 Cookie（需要 {}）。请确认已在登录窗口完成登录，并保持窗口打开。",
            adapter.manifest.name,
            required.join(" / ")
        ));
    }
    if !relevant.iter().any(|cookie| cookie.name() == "csrftoken")
        && pending.adapter_id == platform::YUKETANG_ADAPTER_ID
    {
        tracing::warn!("csrftoken missing after login; continuing with GET-only validation");
    }
    let cookie_header = relevant
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");

    let account_id = pending
        .target_account_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let existing = state.account_by_id(&account_id);

    let session = Session {
        base_url: base_url.clone(),
        region: region_id.clone(),
        cookie_header,
        csrf_token: relevant
            .iter()
            .find(|cookie| cookie.name() == "csrftoken")
            .map(|cookie| cookie.value().to_string()),
        acquired_at: Utc::now().timestamp_millis(),
    };

    let snapshot = crate::refresh::fetch_account_snapshot(
        &app,
        &pending.adapter_id,
        &account_id,
        &session,
    )
    .await
    .map_err(|error| match error {
        refresh::FetchError::AuthExpired => {
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
        adapter_id: pending.adapter_id.clone(),
        region: region_id.clone(),
        base_url: base_url.clone(),
        display_name: existing
            .as_ref()
            .map(|account| account.display_name.clone())
            .unwrap_or(default_display_name),
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
            host == "yuketang.cn"
                || host.ends_with(".yuketang.cn")
                || host == "chaoxing.com"
                || host.ends_with(".chaoxing.com")
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
