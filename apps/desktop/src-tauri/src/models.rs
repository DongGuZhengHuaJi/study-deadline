use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    /// Stable, account-scoped primary key.
    pub id: String,
    pub account_id: String,
    pub remote_id: String,
    pub title: String,
    pub course: String,
    /// assignment | exam | quiz
    pub kind: String,
    /// Absolute deadline, Unix milliseconds.
    pub due_at: Option<i64>,
    /// Exam start time, Unix milliseconds.
    pub starts_at: Option<i64>,
    pub status: String,
    pub completed: bool,
    pub url: String,
    pub source: String,
    pub fetched_at: i64,
}

impl Task {
    /// Time that should drive the countdown and reminder.
    pub fn target_at(&self) -> Option<i64> {
        match self.kind.as_str() {
            "exam" => self.starts_at.or(self.due_at),
            _ => self.due_at,
        }
    }

    pub fn is_remindable(&self) -> bool {
        !self.completed && self.target_at().is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub refresh_minutes: u64,
    pub notifications_enabled: bool,
    pub notify_lead_minutes: Vec<u64>,
    pub autostart: bool,
    pub always_on_top: bool,
    pub risk_accepted: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_minutes: 30,
            notifications_enabled: true,
            notify_lead_minutes: vec![1440, 360, 60, 10],
            autostart: false,
            always_on_top: true,
            risk_accepted: false,
        }
    }
}

impl Settings {
    pub fn normalized(mut self) -> Self {
        const ALLOWED: [u64; 4] = [15, 30, 60, 120];
        if !ALLOWED.contains(&self.refresh_minutes) {
            self.refresh_minutes = 30;
        }
        self.notify_lead_minutes.retain(|value| *value > 0 && *value <= 10080);
        self.notify_lead_minutes.sort_unstable();
        self.notify_lead_minutes.dedup();
        if self.notify_lead_minutes.is_empty() {
            self.notify_lead_minutes = Settings::default().notify_lead_minutes;
        }
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub base_url: String,
    pub region: String,
    pub cookie_header: String,
    pub csrf_token: Option<String>,
    pub acquired_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RegionInfo {
    pub id: String,
    pub label: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AccountView {
    pub id: String,
    pub display_name: String,
    pub region: String,
    pub base_url: String,
    pub authenticated: bool,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncView {
    pub refreshing: bool,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub needs_login: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Dashboard {
    pub tasks: Vec<Task>,
    pub account: Option<AccountView>,
    pub settings: Settings,
    pub sync: SyncView,
    pub regions: Vec<RegionInfo>,
    pub log_path: String,
    pub version: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FetchSnapshot {
    pub tasks: Vec<Task>,
    pub complete: bool,
    pub warnings: Vec<String>,
    pub course_count: usize,
}
