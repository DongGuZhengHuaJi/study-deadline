use crate::models::{FetchSnapshot, RegionInfo, Session, Task};
use chrono::Utc;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

pub const ADAPTER_ID: &str = "yuketang";
pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";
const REQUEST_TIMEOUT_SECS: u64 = 30;
const COURSE_REQUEST_GAP_MS: u64 = 1200;

const REGIONS: [(&str, &str, &str); 4] = [
    ("www", "标准雨课堂", "https://www.yuketang.cn"),
    ("pro", "荷塘雨课堂", "https://pro.yuketang.cn"),
    (
        "changjiang",
        "长江雨课堂",
        "https://changjiang.yuketang.cn",
    ),
    ("huanghe", "黄河雨课堂", "https://huanghe.yuketang.cn"),
];

pub fn regions() -> Vec<RegionInfo> {
    REGIONS
        .iter()
        .map(|(id, label, base_url)| RegionInfo {
            id: id.to_string(),
            label: label.to_string(),
            base_url: base_url.to_string(),
        })
        .collect()
}

pub fn region(id: &str) -> Option<RegionInfo> {
    regions().into_iter().find(|region| region.id == id)
}

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("登录已失效，请重新登录")]
    AuthExpired,
    #[error("请求过于频繁或被平台风控: {0}")]
    RateLimited(String),
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("平台返回异常状态: {0}")]
    Http(u16),
    #[error("平台数据格式变化: {0}")]
    Schema(String),
}

#[derive(Clone, Debug)]
struct Course {
    classroom_id: i64,
    course_id: i64,
    name: String,
    term: Option<i64>,
}

#[derive(Default)]
struct TaskBuilder {
    tasks: Vec<Task>,
    index: HashMap<String, usize>,
}

impl TaskBuilder {
    fn dedupe_key(
        classroom_id: i64,
        title: &str,
        starts_at: Option<i64>,
        due_at: Option<i64>,
    ) -> String {
        format!(
            "{classroom_id}|{}|{}|{}",
            title.trim().to_lowercase(),
            starts_at.unwrap_or(0),
            due_at.unwrap_or(0)
        )
    }

    fn push_or_merge(&mut self, key: String, task: Task) {
        if let Some(index) = self.index.get(&key).copied() {
            if task.kind == "exam" && self.tasks[index].kind != "exam" {
                self.tasks[index].kind = "exam".into();
            }
            if self.tasks[index].starts_at.is_none() {
                self.tasks[index].starts_at = task.starts_at;
            }
            if self.tasks[index].due_at.is_none() {
                self.tasks[index].due_at = task.due_at;
            }
            return;
        }
        self.index.insert(key, self.tasks.len());
        self.tasks.push(task);
    }

    fn finish(mut self) -> Vec<Task> {
        self.tasks.sort_by_key(|task| {
            task.target_at()
                .unwrap_or(i64::MAX)
        });
        self.tasks
    }
}

fn as_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn as_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn clean_text(value: &str, max_chars: usize) -> String {
    let collapsed = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(max_chars).collect()
}

fn status_of(leaf: &Value) -> (String, bool) {
    let progress = leaf.get("progress");
    let status = progress
        .and_then(|p| p.get("status"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("pending")
        .to_string();
    let done = as_i64(progress.and_then(|p| p.get("done"))).unwrap_or(0);
    let total = as_i64(progress.and_then(|p| p.get("total"))).unwrap_or(0);
    let completed = total > 0 && done >= total
        || status.contains("完成")
        || status.contains("已交")
        || status.contains("已批阅");
    (status, completed)
}

fn course_url(base_url: &str, classroom_id: i64) -> String {
    // The student log route uses the classroom/class id, not the course id.
    format!(
        "{}/v2/web/studentLog/{}",
        base_url.trim_end_matches('/'),
        classroom_id
    )
}

fn request_json(
    client: &reqwest::Client,
    session: &Session,
    url: &str,
    classroom_id: Option<i64>,
) -> impl std::future::Future<Output = Result<Value, FetchError>> {
    let request = client
        .get(url)
        .header(reqwest::header::COOKIE, session.cookie_header.clone())
        .header(reqwest::header::REFERER, format!("{}/v2/web/index", session.base_url))
        .header(reqwest::header::ORIGIN, &session.base_url)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(reqwest::header::ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9")
        .header("xtbz", "ykt")
        .header("X-Requested-With", "XMLHttpRequest")
        .header("User-Agent", USER_AGENT);

    let request = if let Some(csrf) = &session.csrf_token {
        request.header("X-CSRFToken", csrf)
    } else {
        request
    };
    let request = if let Some(classroom_id) = classroom_id {
        request.header("classroom-id", classroom_id.to_string())
    } else {
        request
    };

    async move {
        let response = request
            .send()
            .await
            .map_err(|error| FetchError::Network(error.to_string()))?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status.is_redirection()
        {
            return Err(FetchError::AuthExpired);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(FetchError::RateLimited("HTTP 429".into()));
        }
        if !status.is_success() {
            return Err(FetchError::Http(status.as_u16()));
        }
        let text = response
            .text()
            .await
            .map_err(|error| FetchError::Network(error.to_string()))?;
        serde_json::from_str::<Value>(&text)
            .map_err(|error| FetchError::Schema(format!("JSON 解析失败: {error}")))
    }
}

fn parse_courses(value: &Value) -> Result<Vec<Course>, FetchError> {
    let list = value
        .get("data")
        .and_then(|data| data.get("list"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Schema("课程列表缺少 data.list".into()))?;

    let mut courses = Vec::new();
    for item in list {
        let classroom_id = match as_i64(item.get("classroom_id")) {
            Some(value) => value,
            None => continue,
        };
        let course_object = item.get("course");
        let course_id = match as_i64(
            item.get("course_id")
                .or_else(|| course_object.and_then(|course| course.get("id"))),
        ) {
            Some(value) => value,
            None => continue,
        };
        let name = as_string(course_object.and_then(|course| course.get("name")))
            .or_else(|| as_string(item.get("name")))
            .unwrap_or_else(|| "未命名课程".into());
        let term = as_i64(item.get("term"));
        courses.push(Course {
            classroom_id,
            course_id,
            name: clean_text(&name, 120),
            term,
        });
    }
    Ok(courses)
}

fn parse_progress_for_course(
    value: &Value,
    course: &Course,
    account_id: &str,
    base_url: &str,
    builder: &mut TaskBuilder,
    now: i64,
) -> Result<usize, FetchError> {
    let sections = value
        .get("data")
        .and_then(|data| data.get("course_progress"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Schema("课程进度缺少 data.course_progress".into()))?;

    let mut count = 0usize;
    let url = course_url(base_url, course.classroom_id);
    for section in sections {
        let Some(leaves) = section.get("section_leaf_list").and_then(Value::as_array) else {
            continue;
        };
        for leaf in leaves {
            let is_score = leaf
                .get("is_score")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let due_at = as_i64(leaf.get("score_deadline")).filter(|value| *value > 0);
            if !is_score || due_at.is_none() {
                continue;
            }
            let Some(remote_id) = as_i64(
                leaf.get("leafinfo_id")
                    .or_else(|| leaf.get("id")),
            ) else {
                continue;
            };
            let Some(title) = as_string(leaf.get("name")) else {
                continue;
            };
            let title = clean_text(&title, 300);
            if title.is_empty() {
                continue;
            }
            let leaf_type = as_i64(leaf.get("leaf_type")).unwrap_or(5);
            let starts_at = as_i64(leaf.get("start_time")).filter(|value| *value > 0);
            let kind = match leaf_type {
                6 => "exam",
                9 => "quiz",
                _ => "assignment",
            };
            let (status, completed) = status_of(leaf);
            let task = Task {
                id: format!(
                    "{ADAPTER_ID}:{account_id}:leaf:{}:{remote_id}",
                    course.course_id
                ),
                account_id: account_id.to_string(),
                remote_id: format!("leaf:{}:{remote_id}", course.course_id),
                title,
                course: course.name.clone(),
                kind: kind.into(),
                due_at,
                starts_at,
                status,
                completed,
                url: url.clone(),
                source: ADAPTER_ID.into(),
                fetched_at: now,
            };
            let key = TaskBuilder::dedupe_key(
                course.classroom_id,
                &task.title,
                task.starts_at,
                task.due_at,
            );
            builder.push_or_merge(key, task);
            count += 1;
        }
    }
    Ok(count)
}

fn parse_exams(
    value: &Value,
    courses: &[Course],
    account_id: &str,
    base_url: &str,
    builder: &mut TaskBuilder,
    now: i64,
) -> Result<usize, FetchError> {
    let exams = value
        .get("data")
        .and_then(|data| data.get("upcomingExam"))
        .and_then(Value::as_array)
        .ok_or_else(|| FetchError::Schema("考试预告缺少 data.upcomingExam".into()))?;

    let course_by_classroom: HashMap<i64, &Course> =
        courses.iter().map(|course| (course.classroom_id, course)).collect();
    let mut count = 0usize;
    for exam in exams {
        let classroom_id = as_i64(exam.get("classroom_id"));
        let course_name = classroom_id
            .and_then(|id| course_by_classroom.get(&id))
            .map(|course| course.name.clone())
            .or_else(|| as_string(exam.get("classroom_name")))
            .unwrap_or_else(|| "未命名课程".into());
        let Some(title) = as_string(exam.get("title")) else {
            continue;
        };
        let title = clean_text(&title, 300);
        if title.is_empty() {
            continue;
        }
        // The exam endpoint returns seconds; normalise to milliseconds.
        let start = as_i64(exam.get("start_time")).and_then(|seconds| {
            if seconds > 0 {
                Some(seconds.saturating_mul(1000))
            } else {
                None
            }
        });
        let end = as_i64(exam.get("end_time")).and_then(|seconds| {
            if seconds > 0 {
                Some(seconds.saturating_mul(1000))
            } else {
                None
            }
        });
        let due = end.or(start);
        if due.is_none() {
            continue;
        }
        let url = classroom_id
            .map(|id| course_url(base_url, id))
            .unwrap_or_else(|| format!("{base_url}/v2/web/index"));
        let remote_id = format!(
            "exam:{}",
            as_i64(exam.get("id")).unwrap_or_else(|| {
                // Stable fallback derived from the visible metadata.
                let seed = format!("{classroom_id:?}|{title}|{start:?}|{due:?}");
                let mut hash = 0i64;
                for byte in seed.as_bytes() {
                    hash = hash.wrapping_mul(31).wrapping_add(*byte as i64);
                }
                hash
            })
        );
        let task = Task {
            id: format!("{ADAPTER_ID}:{account_id}:{remote_id}"),
            account_id: account_id.to_string(),
            remote_id,
            title,
            course: course_name,
            kind: "exam".into(),
            due_at: due,
            starts_at: start,
            status: "pending".into(),
            completed: false,
            url,
            source: ADAPTER_ID.into(),
            fetched_at: now,
        };
        let classroom = classroom_id.unwrap_or(0);
        let key = TaskBuilder::dedupe_key(
            classroom,
            &task.title,
            task.starts_at,
            task.due_at,
        );
        builder.push_or_merge(key, task);
        count += 1;
    }
    Ok(count)
}

pub async fn fetch_snapshot(
    session: &Session,
    account_id: &str,
) -> Result<FetchSnapshot, FetchError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        // Never forward the Cookie header to a redirect target. A redirect
        // here normally means the session has expired.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| FetchError::Network(format!("创建 HTTP 客户端失败: {error}")))?;

    let base = session.base_url.trim_end_matches('/').to_string();
    let courses_url = format!("{base}/v2/api/web/courses/list?identity=2");
    let courses_value = request_json(&client, session, &courses_url, None).await?;
    let mut courses = parse_courses(&courses_value)?;

    // Only the newest term is relevant for a reminder app; fetching every
    // historical course would multiply requests and old tasks.
    let latest_term = courses
        .iter()
        .filter_map(|course| course.term)
        .filter(|term| *term > 0)
        .max();
    if let Some(latest_term) = latest_term {
        let filtered: Vec<Course> = courses
            .iter()
            .filter(|course| course.term == Some(latest_term))
            .cloned()
            .collect();
        if !filtered.is_empty() {
            courses = filtered;
        }
    }

    let mut warnings = Vec::new();
    let mut complete = true;
    let mut builder = TaskBuilder::default();
    let now = Utc::now().timestamp_millis();
    let mut courses_ok = 0usize;

    for (index, course) in courses.iter().enumerate() {
        let progress_url = format!(
            "{base}/c27/online_courseware/schedule/course/progress/?classroom_id={}&course_id={}",
            course.classroom_id, course.course_id
        );
        match request_json(
            &client,
            session,
            &progress_url,
            Some(course.classroom_id),
        )
        .await
        {
            Ok(value) => {
                courses_ok += 1;
                if let Err(error) =
                    parse_progress_for_course(&value, course, account_id, &base, &mut builder, now)
                {
                    warnings.push(format!("{}: {error}", course.name));
                    complete = false;
                }
            }
            Err(FetchError::AuthExpired) => return Err(FetchError::AuthExpired),
            Err(FetchError::RateLimited(message)) => {
                warnings.push(format!("{}: {message}", course.name));
                complete = false;
                break;
            }
            Err(error) => {
                warnings.push(format!("{}: {error}", course.name));
                complete = false;
            }
        }
        if index + 1 < courses.len() {
            tokio::time::sleep(Duration::from_millis(COURSE_REQUEST_GAP_MS)).await;
        }
    }

    let exams_url = format!("{base}/api/v3/classroom/on-lesson-upcoming-exam");
    match request_json(&client, session, &exams_url, None).await {
        Ok(value) => {
            if let Err(error) =
                parse_exams(&value, &courses, account_id, &base, &mut builder, now)
            {
                warnings.push(format!("考试预告: {error}"));
                complete = false;
            }
        }
        Err(FetchError::AuthExpired) => return Err(FetchError::AuthExpired),
        Err(error) => {
            warnings.push(format!("考试预告: {error}"));
            complete = false;
        }
    }

    if !courses.is_empty() && courses_ok == 0 {
        return Err(FetchError::Network(
            "当前学期课程全部拉取失败，已保留旧任务".into(),
        ));
    }

    Ok(FetchSnapshot {
        tasks: builder.finish(),
        complete,
        warnings,
        course_count: courses.len(),
    })
}
