use crate::models::{Settings, Task};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct AccountRow {
    pub id: String,
    pub adapter_id: String,
    pub region: String,
    pub base_url: String,
    pub display_name: String,
    pub session_blob: Option<Vec<u8>>,
    pub created_at: i64,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|error| format!("打开数据库失败: {error}"))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                adapter_id TEXT NOT NULL,
                region TEXT NOT NULL,
                base_url TEXT NOT NULL,
                display_name TEXT NOT NULL,
                session_blob BLOB,
                created_at INTEGER NOT NULL,
                last_success_at INTEGER,
                last_error TEXT
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                account_id TEXT NOT NULL,
                remote_id TEXT NOT NULL,
                title TEXT NOT NULL,
                course TEXT NOT NULL,
                kind TEXT NOT NULL,
                due_at INTEGER,
                starts_at INTEGER,
                status TEXT NOT NULL,
                completed INTEGER NOT NULL DEFAULT 0,
                url TEXT NOT NULL,
                source TEXT NOT NULL,
                fetched_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_account_target
                ON tasks(account_id, due_at, starts_at);
            CREATE TABLE IF NOT EXISTS notifications (
                task_id TEXT NOT NULL,
                threshold_minutes INTEGER NOT NULL,
                sent_at INTEGER NOT NULL,
                PRIMARY KEY (task_id, threshold_minutes)
            );
            "#,
        )
        .map_err(|error| format!("初始化数据库失败: {error}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
        self.conn
            .lock()
            .map_err(|_| "数据库状态损坏".to_string())
    }

    pub fn load_settings(&self) -> Settings {
        self.lock()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT value FROM settings WHERE key = 'app'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .ok()
                .flatten()
            })
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
            .unwrap_or_default()
            .normalized()
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        let raw = serde_json::to_string(settings).map_err(|error| error.to_string())?;
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO settings(key, value) VALUES('app', ?1)",
                params![raw],
            )
            .map_err(|error| format!("保存设置失败: {error}"))?;
        Ok(())
    }

    pub fn load_account(&self, adapter_id: &str) -> Option<AccountRow> {
        self.lock()
            .ok()?
            .query_row(
                "SELECT id, adapter_id, region, base_url, display_name, session_blob,
                        created_at, last_success_at, last_error
                 FROM accounts WHERE adapter_id = ?1
                 ORDER BY COALESCE(last_success_at, 0) DESC LIMIT 1",
                params![adapter_id],
                |row| {
                    Ok(AccountRow {
                        id: row.get(0)?,
                        adapter_id: row.get(1)?,
                        region: row.get(2)?,
                        base_url: row.get(3)?,
                        display_name: row.get(4)?,
                        session_blob: row.get(5)?,
                        created_at: row.get(6)?,
                        last_success_at: row.get(7)?,
                        last_error: row.get(8)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn save_account(&self, account: &AccountRow) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR REPLACE INTO accounts(
                    id, adapter_id, region, base_url, display_name, session_blob,
                    created_at, last_success_at, last_error
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    account.id,
                    account.adapter_id,
                    account.region,
                    account.base_url,
                    account.display_name,
                    account.session_blob,
                    account.created_at,
                    account.last_success_at,
                    account.last_error,
                ],
            )
            .map_err(|error| format!("保存账号失败: {error}"))?;
        Ok(())
    }

    pub fn update_session_blob(&self, account_id: &str, blob: Option<&[u8]>) -> Result<(), String> {
        self.lock()?
            .execute(
                "UPDATE accounts SET session_blob = ?1 WHERE id = ?2",
                params![blob, account_id],
            )
            .map_err(|error| format!("保存会话失败: {error}"))?;
        Ok(())
    }

    pub fn mark_success(&self, account_id: &str, at: i64) -> Result<(), String> {
        self.lock()?
            .execute(
                "UPDATE accounts SET last_success_at = ?1, last_error = NULL WHERE id = ?2",
                params![at, account_id],
            )
            .map_err(|error| format!("更新账号状态失败: {error}"))?;
        Ok(())
    }

    pub fn mark_error(&self, account_id: &str, message: &str) -> Result<(), String> {
        self.lock()?
            .execute(
                "UPDATE accounts SET last_error = ?1 WHERE id = ?2",
                params![message, account_id],
            )
            .map_err(|error| format!("更新账号状态失败: {error}"))?;
        Ok(())
    }

    pub fn replace_tasks(
        &self,
        account_id: &str,
        tasks: &[Task],
        complete: bool,
    ) -> Result<(), String> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|error| format!("开始数据库事务失败: {error}"))?;
        if complete {
            tx.execute(
                "DELETE FROM tasks WHERE account_id = ?1",
                params![account_id],
            )
            .map_err(|error| format!("清理旧任务失败: {error}"))?;
        }
        for task in tasks {
            tx.execute(
                "INSERT OR REPLACE INTO tasks(
                    id, account_id, remote_id, title, course, kind, due_at, starts_at,
                    status, completed, url, source, fetched_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    task.id,
                    task.account_id,
                    task.remote_id,
                    task.title,
                    task.course,
                    task.kind,
                    task.due_at,
                    task.starts_at,
                    task.status,
                    task.completed as i64,
                    task.url,
                    task.source,
                    task.fetched_at,
                ],
            )
            .map_err(|error| format!("写入任务失败: {error}"))?;
        }
        tx.commit()
            .map_err(|error| format!("提交任务失败: {error}"))?;
        Ok(())
    }

    pub fn load_tasks(&self, account_id: &str) -> Result<Vec<Task>, String> {
        let conn = self.lock()?;
        let mut statement = conn
            .prepare(
                "SELECT id, account_id, remote_id, title, course, kind, due_at, starts_at,
                        status, completed, url, source, fetched_at
                 FROM tasks WHERE account_id = ?1
                 ORDER BY COALESCE(starts_at, due_at, 9223372036854775807) ASC",
            )
            .map_err(|error| format!("读取任务失败: {error}"))?;
        let rows = statement
            .query_map(params![account_id], |row| {
                Ok(Task {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    remote_id: row.get(2)?,
                    title: row.get(3)?,
                    course: row.get(4)?,
                    kind: row.get(5)?,
                    due_at: row.get(6)?,
                    starts_at: row.get(7)?,
                    status: row.get(8)?,
                    completed: row.get::<_, i64>(9)? != 0,
                    url: row.get(10)?,
                    source: row.get(11)?,
                    fetched_at: row.get(12)?,
                })
            })
            .map_err(|error| format!("读取任务失败: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取任务失败: {error}"))
    }

    pub fn task_by_id(&self, task_id: &str) -> Option<Task> {
        self.lock()
            .ok()?
            .query_row(
                "SELECT id, account_id, remote_id, title, course, kind, due_at, starts_at,
                        status, completed, url, source, fetched_at
                 FROM tasks WHERE id = ?1",
                params![task_id],
                |row| {
                    Ok(Task {
                        id: row.get(0)?,
                        account_id: row.get(1)?,
                        remote_id: row.get(2)?,
                        title: row.get(3)?,
                        course: row.get(4)?,
                        kind: row.get(5)?,
                        due_at: row.get(6)?,
                        starts_at: row.get(7)?,
                        status: row.get(8)?,
                        completed: row.get::<_, i64>(9)? != 0,
                        url: row.get(10)?,
                        source: row.get(11)?,
                        fetched_at: row.get(12)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn delete_account(&self, account_id: &str) -> Result<(), String> {
        let mut conn = self.lock()?;
        let tx = conn
            .transaction()
            .map_err(|error| format!("开始数据库事务失败: {error}"))?;
        tx.execute("DELETE FROM tasks WHERE account_id = ?1", params![account_id])
            .map_err(|error| format!("删除任务失败: {error}"))?;
        tx.execute("DELETE FROM accounts WHERE id = ?1", params![account_id])
            .map_err(|error| format!("删除账号失败: {error}"))?;
        tx.execute(
            "DELETE FROM notifications WHERE task_id NOT IN (SELECT id FROM tasks)",
            [],
        )
        .map_err(|error| format!("清理提醒记录失败: {error}"))?;
        tx.commit()
            .map_err(|error| format!("提交删除失败: {error}"))?;
        Ok(())
    }

    pub fn was_notified(&self, task_id: &str, threshold_minutes: u64) -> bool {
        self.lock()
            .ok()
            .and_then(|conn| {
                conn.query_row(
                    "SELECT 1 FROM notifications WHERE task_id = ?1 AND threshold_minutes = ?2",
                    params![task_id, threshold_minutes as i64],
                    |_| Ok(()),
                )
                .optional()
                .ok()
                .flatten()
            })
            .is_some()
    }

    pub fn mark_notified(
        &self,
        task_id: &str,
        threshold_minutes: u64,
        at: i64,
    ) -> Result<(), String> {
        self.lock()?
            .execute(
                "INSERT OR IGNORE INTO notifications(task_id, threshold_minutes, sent_at)
                 VALUES(?1, ?2, ?3)",
                params![task_id, threshold_minutes as i64, at],
            )
            .map_err(|error| format!("记录提醒失败: {error}"))?;
        Ok(())
    }
}
