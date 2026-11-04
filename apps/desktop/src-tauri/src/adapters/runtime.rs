use super::manifest::{LoadedAdapter, CURRENT_API_VERSION};
use crate::models::{FetchSnapshot, Session, Task};
use chrono::Utc;
use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

const RUN_TIMEOUT: Duration = Duration::from_secs(90);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_HTTP_BODY: usize = 5 * 1024 * 1024;
const MAX_TASKS: usize = 2000;
const JS_MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const JS_STACK_LIMIT: usize = 512 * 1024;

#[derive(Debug, Error)]
pub enum AdapterRunError {
    #[error("适配器 manifest 不兼容: {0}")]
    Manifest(String),
    #[error("适配器运行时错误: {0}")]
    Runtime(String),
    #[error("适配器返回数据无效: {0}")]
    InvalidOutput(String),
    #[error("适配器网络请求失败: {0}")]
    Network(String),
    #[error("登录已失效")]
    AuthExpired,
    #[error("平台请求被限流")]
    RateLimited,
    #[error("平台数据结构变化: {0}")]
    SchemaChanged(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostConfig {
    adapter_id: String,
    region: String,
    base_url: String,
    timezone: String,
    locale: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostHttpRequest {
    url: String,
    #[serde(default = "default_get")]
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

fn default_get() -> String {
    "GET".into()
}

#[derive(Debug, Serialize)]
struct HostHttpResponse {
    status: u16,
    body: String,
    /// Present for redirects because the host deliberately does not follow
    /// them itself. Adapters may follow same-platform redirects manually.
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdapterFetchResult {
    #[serde(default)]
    tasks: Vec<AdapterTask>,
    #[serde(default)]
    complete: bool,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdapterTask {
    remote_id: String,
    title: String,
    #[serde(default)]
    course: String,
    #[serde(default = "default_assignment")]
    kind: String,
    #[serde(default)]
    due_at: Option<i64>,
    #[serde(default)]
    starts_at: Option<i64>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    url: Option<String>,
}

fn default_assignment() -> String {
    "assignment".into()
}

struct HostState {
    manifest_id: String,
    allowed_http: Vec<String>,
    allowed_session: HashSet<String>,
    cookie_header: String,
    config: HostConfig,
    client: reqwest::blocking::Client,
}

impl HostState {
    fn new(
        adapter: &LoadedAdapter,
        session: Option<&Session>,
    ) -> Result<Self, AdapterRunError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(crate::platform::USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| AdapterRunError::Network(error.to_string()))?;
        let (base_url, region, cookie_header, region_timezone) = session
            .map(|session| {
                (
                    session.base_url.clone(),
                    session.region.clone(),
                    session.cookie_header.clone(),
                    "Asia/Shanghai".to_string(),
                )
            })
            .unwrap_or_else(|| (String::new(), String::new(), String::new(), "Asia/Shanghai".into()));
        let allowed_session = adapter
            .manifest
            .permissions
            .session
            .iter()
            .map(|entry| normalize_session_key(entry))
            .collect();
        Ok(Self {
            manifest_id: adapter.manifest.id.clone(),
            allowed_http: adapter.manifest.permissions.http.clone(),
            allowed_session,
            cookie_header,
            config: HostConfig {
                adapter_id: adapter.manifest.id.clone(),
                region,
                base_url,
                timezone: region_timezone,
                locale: "zh-CN".into(),
            },
            client,
        })
    }

    fn config_json(&self) -> String {
        serde_json::to_string(&self.config).unwrap_or_else(|_| "{}".into())
    }

    fn session_get(&self, key: String) -> String {
        let key = normalize_session_key(&key);
        if !self.allowed_session.contains(&key) {
            return String::new();
        }
        cookie_value(&self.cookie_header, &key).unwrap_or_default()
    }

    fn sleep(&self, milliseconds: u32) {
        let milliseconds = milliseconds.min(5000);
        std::thread::sleep(Duration::from_millis(milliseconds as u64));
    }

    fn log(&self, level: String, message: String) {
        let message: String = message.chars().take(500).collect();
        match level.as_str() {
            "error" => tracing::warn!(adapter = %self.manifest_id, "{message}"),
            "warn" => tracing::warn!(adapter = %self.manifest_id, "{message}"),
            "debug" => tracing::debug!(adapter = %self.manifest_id, "{message}"),
            _ => tracing::info!(adapter = %self.manifest_id, "{message}"),
        }
    }

    fn http(&self, input: String) -> String {
        match self.try_http(&input) {
            Ok(response) => serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"error":"serialize failed"}"#.into()),
            Err(error) => serde_json::json!({ "error": error }).to_string(),
        }
    }

    fn try_http(&self, input: &str) -> Result<HostHttpResponse, String> {
        let request: HostHttpRequest = serde_json::from_str(input)
            .map_err(|error| format!("invalid request JSON: {error}"))?;
        let url = url::Url::parse(&request.url).map_err(|error| format!("invalid URL: {error}"))?;
        if url.scheme() != "https" {
            return Err("only HTTPS is allowed".into());
        }
        if !url_allowed(&request.url, &self.allowed_http) {
            // Do not put query strings (which may contain opaque tokens such
            // as work/detail `enc` values) into logs or UI errors.
            let safe = format!(
                "{}://{}{}",
                url.scheme(),
                url.host_str().unwrap_or("?"),
                url.path()
            );
            return Err(format!("URL not allowed by manifest: {safe}"));
        }
        let method = match request.method.to_ascii_uppercase().as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            _ => return Err("only GET and POST are allowed".into()),
        };
        let mut builder = self
            .client
            .request(method, url)
            .header(reqwest::header::COOKIE, &self.cookie_header);
        for (name, value) in request.headers {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "cookie" | "host" | "content-length" | "user-agent"
            ) {
                return Err(format!("header is managed by host: {name}"));
            }
            builder = builder.header(name, value);
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .send()
            .map_err(|error| format!("network error: {}", error.without_url()))?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response
            .bytes()
            .map_err(|error| format!("read response failed: {}", error.without_url()))?;
        if bytes.len() > MAX_HTTP_BODY {
            return Err("response too large".into());
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        Ok(HostHttpResponse {
            status,
            body,
            location,
        })
    }
}

fn normalize_session_key(key: &str) -> String {
    key.trim()
        .strip_prefix("cookie:")
        .or_else(|| key.trim().strip_prefix("localStorage:"))
        .unwrap_or(key.trim())
        .to_string()
}

fn cookie_value(cookie_header: &str, key: &str) -> Option<String> {
    cookie_header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

/// True when a URL is within the adapter manifest's HTTP permission set.
pub fn url_allowed_for_adapter(adapter: &LoadedAdapter, url: &str) -> bool {
    url_allowed(url, &adapter.manifest.permissions.http)
}

/// A deliberately conservative URL allowlist. The manifest pattern is an
/// HTTPS origin plus optional path prefix, with `*.` subdomain wildcards.
fn url_allowed(url: &str, patterns: &[String]) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https"
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let path = parsed.path();
    patterns.iter().any(|pattern| {
        let Some(rest) = pattern.strip_prefix("https://") else {
            return false;
        };
        let (host_pattern, path_pattern) = rest.split_once('/').unwrap_or((rest, ""));
        let host_matches = if let Some(suffix) = host_pattern.strip_prefix("*.") {
            host.len() > suffix.len()
                && host.ends_with(suffix)
                && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
        } else {
            host.eq_ignore_ascii_case(host_pattern)
        };
        if !host_matches {
            return false;
        }
        if path_pattern.is_empty() || path_pattern == "*" {
            return true;
        }
        let prefix = path_pattern.trim_end_matches('*');
        let prefix = if prefix.starts_with('/') {
            prefix.to_string()
        } else {
            format!("/{prefix}")
        };
        path.starts_with(&prefix)
    })
}

fn validate_task(
    adapter: &LoadedAdapter,
    account_id: &str,
    task: AdapterTask,
    now: i64,
) -> Result<Option<Task>, String> {
    let remote_id = task.remote_id.trim();
    let title = task.title.trim();
    if remote_id.is_empty() || title.is_empty() {
        return Ok(None);
    }
    let remote_id = remote_id.chars().take(200).collect::<String>();
    let title = title.chars().take(300).collect::<String>();
    let course = if task.course.trim().is_empty() {
        "未命名课程".to_string()
    } else {
        task.course.trim().chars().take(120).collect()
    };
    let kind = match task.kind.as_str() {
        "assignment" | "exam" | "quiz" => task.kind,
        other => return Err(format!("unknown task kind: {other}")),
    };
    let due_at = task.due_at.filter(|value| *value > 0);
    let starts_at = task.starts_at.filter(|value| *value > 0);
    if due_at.is_none() && starts_at.is_none() {
        return Ok(None);
    }
    let url = task.url.unwrap_or_default();
    if !url.is_empty() {
        if !url_allowed_for_adapter(adapter, &url) {
            return Err("task URL is not allowed by manifest".into());
        }
    }
    Ok(Some(Task {
        id: format!("{}:{}:{}", adapter.manifest.id, account_id, remote_id),
        account_id: account_id.to_string(),
        remote_id,
        title,
        course,
        kind,
        due_at,
        starts_at,
        status: task.status.unwrap_or_else(|| "pending".into()),
        completed: task.completed,
        url,
        source: adapter.manifest.id.clone(),
        fetched_at: now,
    }))
}

/// Execute one adapter synchronously. The caller must run this on a dedicated
/// blocking thread; QuickJS is intentionally not exposed to the async runtime.
pub fn run_adapter(
    adapter: &LoadedAdapter,
    session: Option<&Session>,
    account_id: &str,
) -> Result<FetchSnapshot, AdapterRunError> {
    if adapter.manifest.api_version != CURRENT_API_VERSION {
        return Err(AdapterRunError::Manifest(
            "adapter API version mismatch".into(),
        ));
    }
    let host = Arc::new(HostState::new(adapter, session)?);
    let runtime = Runtime::new().map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
    runtime.set_memory_limit(JS_MEMORY_LIMIT);
    runtime.set_max_stack_size(JS_STACK_LIMIT);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || {
        started.elapsed() > RUN_TIMEOUT
    })));
    let context =
        Context::full(&runtime).map_err(|error| AdapterRunError::Runtime(error.to_string()))?;

    let result = context.with(|ctx| {
        let globals = ctx.globals();
        let host_http = {
            let host = host.clone();
            Function::new(ctx.clone(), move |input: String| host.http(input))
                .map_err(|error| AdapterRunError::Runtime(error.to_string()))?
        };
        let host_session = {
            let host = host.clone();
            Function::new(ctx.clone(), move |key: String| host.session_get(key))
                .map_err(|error| AdapterRunError::Runtime(error.to_string()))?
        };
        let host_log = {
            let host = host.clone();
            Function::new(ctx.clone(), move |level: String, message: String| {
                host.log(level, message);
            })
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?
        };
        let host_sleep = {
            let host = host.clone();
            Function::new(ctx.clone(), move |milliseconds: u32| {
                host.sleep(milliseconds);
            })
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?
        };
        globals
            .set("__host_http", host_http)
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
        globals
            .set("__host_session_get", host_session)
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
        globals
            .set("__host_log", host_log)
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
        globals
            .set("__host_sleep", host_sleep)
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
        globals
            .set("__host_config_json", host.config_json())
            .map_err(|error| AdapterRunError::Runtime(error.to_string()))?;
        ctx.eval::<(), _>(PRELUDE)
            .map_err(|error| AdapterRunError::Runtime(format!("SDK prelude failed: {error}")))?;
        ctx.eval::<(), _>(adapter.entry_source.as_str()).map_err(|error| {
            AdapterRunError::Runtime(format!("adapter load failed: {error}"))
        })?;
        let has_adapter: bool = ctx
            .eval("typeof globalThis.adapter === 'object' && typeof globalThis.adapter.fetchTasks === 'function'")
            .map_err(|error| AdapterRunError::Runtime(format!("invalid adapter: {error}")))?;
        if !has_adapter {
            return Err(AdapterRunError::Runtime(
                "adapter must define globalThis.adapter.fetchTasks(ctx)".into(),
            ));
        }
        let raw: String = ctx
            .eval("JSON.stringify(globalThis.adapter.fetchTasks(globalThis.__ctx))")
            .map_err(|error| {
                AdapterRunError::Runtime(format!("fetchTasks failed: {error}"))
            })?;
        let parsed: AdapterFetchResult = serde_json::from_str(&raw)
            .map_err(|error| AdapterRunError::InvalidOutput(error.to_string()))?;
        if let Some(error) = parsed.error.as_deref() {
            return Err(match error {
                "AUTH_EXPIRED" => AdapterRunError::AuthExpired,
                "RATE_LIMITED" | "RISK_CONTROL" => AdapterRunError::RateLimited,
                "NETWORK" | "HTTP_ERROR" => {
                    AdapterRunError::Network(format!("adapter reported: {error}"))
                }
                "SCHEMA_CHANGED" => {
                    AdapterRunError::SchemaChanged("adapter reported schema change".into())
                }
                other => AdapterRunError::Runtime(format!("adapter error: {other}")),
            });
        }
        if parsed.tasks.len() > MAX_TASKS {
            return Err(AdapterRunError::InvalidOutput("too many tasks".into()));
        }
        let now = Utc::now().timestamp_millis();
        let mut seen = HashSet::new();
        let mut tasks = Vec::new();
        for task in parsed.tasks {
            if let Some(task) = validate_task(adapter, account_id, task, now)
                .map_err(AdapterRunError::InvalidOutput)?
            {
                if seen.insert(task.id.clone()) {
                    tasks.push(task);
                }
            }
        }
        tasks.sort_by_key(|task| task.target_at().unwrap_or(i64::MAX));
        Ok(FetchSnapshot {
            tasks,
            complete: parsed.complete,
            warnings: parsed.warnings,
        })
    });
    result
}

const PRELUDE: &str = r#"
(() => {
  const parse = (text) => {
    try { return JSON.parse(text); }
    catch (error) { return { error: String(error) }; }
  };
  const ctx = {
    http: {
      request(request) {
        return parse(globalThis.__host_http(JSON.stringify(request)));
      },
      getJson(url, options) {
        const response = ctx.http.request(Object.assign({}, options, { url, method: "GET" }));
        if (response.error) throw new Error(response.error);
        if (response.status < 200 || response.status >= 300) {
          throw new Error("HTTP " + response.status);
        }
        return JSON.parse(response.body);
      },
      getText(url, options) {
        const response = ctx.http.request(Object.assign({}, options, { url, method: "GET" }));
        if (response.error) throw new Error(response.error);
        if (response.status < 200 || response.status >= 300) {
          throw new Error("HTTP " + response.status);
        }
        return response.body;
      }
    },
    sleep(milliseconds) {
      globalThis.__host_sleep(Math.max(0, Math.min(5000, Number(milliseconds) || 0)));
    },
    session: {
      get(key) {
        const value = globalThis.__host_session_get(String(key));
        return value === "" ? null : value;
      }
    },
    log: {
      debug(message) { globalThis.__host_log("debug", String(message)); },
      info(message) { globalThis.__host_log("info", String(message)); },
      warn(message) { globalThis.__host_log("warn", String(message)); },
      error(message) { globalThis.__host_log("error", String(message)); }
    },
    config: parse(globalThis.__host_config_json)
  };
  globalThis.__ctx = ctx;
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn runs_mock_adapter() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../adapters/mock");
        let adapter = super::super::manifest::load_adapter_dir(&root, "builtin").unwrap();
        let result = run_adapter(&adapter, None, "smoke").unwrap();
        assert_eq!(result.tasks.len(), 1);
        assert!(result.complete);
        assert_eq!(result.tasks[0].source, "mock");
    }

    fn eval_adapter_source(adapter_id: &str) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapters")
            .join(adapter_id);
        let adapter = super::super::manifest::load_adapter_dir(&root, "builtin").unwrap();
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            ctx.eval::<(), _>(adapter.entry_source.as_str()).unwrap();
            let ok: bool = ctx
                .eval("typeof globalThis.adapter === 'object' && typeof globalThis.adapter.fetchTasks === 'function'")
                .unwrap();
            assert!(ok, "adapter {adapter_id} did not load");
        });
    }

    #[test]
    fn loads_yuketang_adapter_source() {
        eval_adapter_source("yuketang");
    }

    #[test]
    fn loads_chaoxing_adapter_source() {
        eval_adapter_source("chaoxing");
    }

    #[test]
    fn chaoxing_task_urls_are_allowed() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../adapters/chaoxing");
        let adapter = super::super::manifest::load_adapter_dir(&root, "builtin").unwrap();
        assert!(url_allowed_for_adapter(
            &adapter,
            "https://mooc1-2.chaoxing.com/visit/stucoursemiddle?courseid=1&clazzid=2&vc=1"
        ));
        assert!(url_allowed_for_adapter(
            &adapter,
            "https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?fid=1"
        ));
        assert!(!url_allowed_for_adapter(
            &adapter,
            "https://example.com/not-chaoxing"
        ));
    }

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../adapters/chaoxing/tests/fixtures")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn runs_chaoxing_adapter_against_fixtures() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../adapters/chaoxing");
        let adapter = super::super::manifest::load_adapter_dir(&root, "builtin").unwrap();
        let runtime = Runtime::new().unwrap();
        let context = Context::full(&runtime).unwrap();
        context.with(|ctx| {
            let globals = ctx.globals();
            let http = Function::new(ctx.clone(), |input: String| {
                let request: serde_json::Value = serde_json::from_str(&input).unwrap();
                let url = request["url"].as_str().unwrap_or_default();
                let body = if url.starts_with("https://i.chaoxing.com/base") {
                    fixture("base.html")
                } else if url.contains("/mycourse/backclazzdata") {
                    fixture("courses.json")
                } else if url.starts_with("https://mooc1-api.chaoxing.com/work/stu-work") {
                    fixture("stu-work.html")
                } else if url.starts_with(
                    "https://mooc1.chaoxing.com/mooc-ans/mooc2/work/task",
                ) {
                    fixture("work-detail-9001.html")
                } else if url.contains("/exam-ans/exam/test/examcode/examlist") {
                    fixture("examlist.html")
                } else {
                    return serde_json::json!({ "error": "unexpected URL" }).to_string();
                };
                serde_json::json!({ "status": 200, "body": body }).to_string()
            })
            .unwrap();
            let session =
                Function::new(ctx.clone(), |_key: String| "1234".to_string()).unwrap();
            let log = Function::new(ctx.clone(), |_level: String, _message: String| {}).unwrap();
            let sleep = Function::new(ctx.clone(), |_milliseconds: u32| {}).unwrap();
            globals.set("__host_http", http).unwrap();
            globals.set("__host_session_get", session).unwrap();
            globals.set("__host_log", log).unwrap();
            globals.set("__host_sleep", sleep).unwrap();
            globals
                .set(
                    "__host_config_json",
                    r#"{"adapterId":"chaoxing","region":"default","baseUrl":"https://i.chaoxing.com","timezone":"Asia/Shanghai","locale":"zh-CN"}"#,
                )
                .unwrap();
            ctx.eval::<(), _>(PRELUDE).unwrap();
            ctx.eval::<(), _>(adapter.entry_source.as_str()).unwrap();
            let raw: String = ctx
                .eval("JSON.stringify(globalThis.adapter.fetchTasks(globalThis.__ctx))")
                .unwrap();
            let parsed: AdapterFetchResult = serde_json::from_str(&raw).unwrap();
            assert_eq!(parsed.error, None);
            assert!(parsed.complete);
            assert_eq!(parsed.tasks.len(), 4);
            let work = parsed
                .tasks
                .iter()
                .find(|task| task.title == "第三次作业")
                .unwrap();
            assert_eq!(work.course, "概率论");
            assert_eq!(work.due_at, Some(1_782_043_200_000));
            let exam = parsed
                .tasks
                .iter()
                .find(|task| task.title == "信号与系统期末考试")
                .unwrap();
            assert_eq!(exam.kind, "exam");
            assert_eq!(exam.starts_at, Some(1_782_867_600_000));
            assert_eq!(exam.due_at, Some(1_782_874_800_000));
        });
    }

    #[test]
    fn url_allowlist_blocks_other_hosts() {
        let patterns = vec!["https://*.example.com/*".to_string()];
        assert!(url_allowed("https://api.example.com/v1", &patterns));
        assert!(!url_allowed("https://example.org/v1", &patterns));
        assert!(!url_allowed("http://api.example.com/v1", &patterns));
        assert!(!url_allowed("https://api.example.com.evil.test/v1", &patterns));
    }

    #[test]
    fn url_allowlist_supports_path_prefix() {
        let patterns = vec!["https://api.example.com/v1/*".to_string()];
        assert!(url_allowed("https://api.example.com/v1/tasks", &patterns));
        assert!(!url_allowed("https://api.example.com/v2/tasks", &patterns));
    }

    #[test]
    fn cookie_parse() {
        assert_eq!(
            cookie_value("sessionid=abc; csrftoken=def", "csrftoken"),
            Some("def".into())
        );
    }
}
