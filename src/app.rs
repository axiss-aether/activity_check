use std::{
    collections::HashMap,
    env, fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Local, NaiveDate};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use tauri::{
    Manager, State, WindowEvent,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use windows_sys::Win32::{
    System::SystemInformation::GetTickCount,
    UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};
use winrt_notification::Toast;

const IDLE_LIMIT_SECONDS: u64 = 5 * 60;
const SAVE_INTERVAL: Duration = Duration::from_secs(5);
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ActivityCheck";
const ACCENTS: [&str; 6] = [
    "#52855B", "#00BECD", "#2378FF", "#7E53D6", "#ED5B93", "#F68930",
];

pub type SharedTracker = Arc<Mutex<Tracker>>;

pub struct Tracker {
    db: Connection,
    session_id: i64,
    session_day: NaiveDate,
    session_started: DateTime<Local>,
    active_seconds: u64,
    session_active_seconds: u64,
    idle_seconds: u64,
    active_now: bool,
    last_tick: Instant,
    last_save: Instant,
    layout_type: String,
    theme: String,
    accent_index: usize,
    goal_hours: u16,
    notify_goal: bool,
    pause_minutes: u8,
    goal_notified_day: Option<String>,
    autostart_enabled: bool,
    db_error: Option<String>,
    status_message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub day: String,
    pub started_at: String,
    pub active_seconds: u64,
    pub ended_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeekDay {
    pub date: String,
    pub label: String,
    pub seconds: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub today: String,
    pub started_at: String,
    pub active_seconds: u64,
    pub today_seconds: u64,
    pub idle_seconds: u64,
    pub active_now: bool,
    pub goal_hours: u16,
    pub goal_seconds: u64,
    pub notify_goal: bool,
    pub pause_minutes: u8,
    pub layout_type: String,
    pub theme: String,
    pub accent_index: usize,
    pub accent_color: String,
    pub autostart_enabled: bool,
    pub history: Vec<SessionRow>,
    pub week: Vec<WeekDay>,
    pub db_error: Option<String>,
    pub status_message: Option<String>,
}

impl Tracker {
    pub fn new() -> Result<Self, String> {
        let path = database_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Не удалось создать папку базы данных: {error}"))?;
        }
        let db = Connection::open(path)
            .map_err(|error| format!("Не удалось открыть SQLite: {error}"))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS sessions (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 day TEXT NOT NULL,
                 started_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 ended_at TEXT,
                 active_seconds INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_sessions_day ON sessions(day);
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(|error| format!("Не удалось подготовить SQLite: {error}"))?;
        ensure_updated_at_column(&db)
            .map_err(|error| format!("Не удалось обновить схему SQLite: {error}"))?;

        let now = Local::now();
        db.execute(
            "UPDATE sessions SET ended_at = COALESCE(NULLIF(updated_at, ''), started_at) WHERE ended_at IS NULL",
            [],
        ).map_err(|error| format!("Не удалось закрыть предыдущую сессию: {error}"))?;
        merge_sessions_by_day(&db)
            .map_err(|error| format!("Не удалось объединить записи активности по дням: {error}"))?;

        let autostart_enabled = load_setting(&db, "autostart", "1")? == "1";
        let status_message = set_autostart(autostart_enabled)
            .err()
            .map(|error| format!("Не удалось настроить автозапуск: {error}"));
        save_setting(&db, "autostart", bool_setting(autostart_enabled))
            .map_err(|error| format!("Не удалось сохранить настройки: {error}"))?;

        let saved_layout = load_setting(&db, "layout_type", "quiet")?;
        let layout_type = if ["quiet", "bento", "heatmap"].contains(&saved_layout.as_str()) {
            saved_layout
        } else {
            "quiet".to_owned()
        };
        let theme = match load_setting(&db, "theme", "light")?.as_str() {
            "dark" => "dark".to_owned(),
            _ => "light".to_owned(),
        };
        let accent_index = load_setting(&db, "accent_index", "0")?
            .parse::<usize>()
            .unwrap_or(0)
            .min(ACCENTS.len() - 1);
        let goal_hours = load_setting(&db, "goal_hours", "5")?
            .parse::<u16>()
            .unwrap_or(5)
            .clamp(1, 24);
        let notify_goal = load_setting(&db, "notify_goal", "1")? == "1";
        let pause_minutes = load_setting(&db, "pause_minutes", "5")?
            .parse::<u8>()
            .unwrap_or(5)
            .min(60);
        let notified = load_setting(&db, "goal_notified_day", "")?;
        let goal_notified_day = (!notified.is_empty()).then_some(notified);
        let session_day = now.date_naive();
        let (session_id, active_seconds) = start_or_resume_day(&db, &now)
            .map_err(|error| format!("Не удалось открыть запись за сегодня: {error}"))?;
        let idle_seconds = idle_seconds().unwrap_or(IDLE_LIMIT_SECONDS);
        let active_now = pause_minutes == 0 || idle_seconds < u64::from(pause_minutes) * 60;

        Ok(Self {
            db,
            session_id,
            session_day,
            session_started: now,
            active_seconds,
            session_active_seconds: 0,
            idle_seconds,
            active_now,
            last_tick: Instant::now(),
            last_save: Instant::now(),
            layout_type,
            theme,
            accent_index,
            goal_hours,
            notify_goal,
            pause_minutes,
            goal_notified_day,
            autostart_enabled,
            db_error: None,
            status_message,
        })
    }

    fn tick(&mut self) {
        let instant = Instant::now();
        let now = Local::now();
        if now.date_naive() != self.session_day {
            if let Err(error) = self.roll_session(now) {
                self.db_error = Some(error);
            }
        }
        let elapsed = instant.duration_since(self.last_tick).as_secs().clamp(1, 2);
        self.last_tick = instant;
        match idle_seconds() {
            Some(idle) => {
                self.idle_seconds = idle;
                self.active_now =
                    self.pause_minutes == 0 || idle < u64::from(self.pause_minutes) * 60;
                if self.active_now {
                    self.active_seconds = self.active_seconds.saturating_add(elapsed);
                    self.session_active_seconds =
                        self.session_active_seconds.saturating_add(elapsed);
                }
            }
            None => {
                self.active_now = false;
                self.status_message =
                    Some("Windows не смогла определить время последнего ввода.".to_owned());
            }
        }
        if instant.duration_since(self.last_save) >= SAVE_INTERVAL {
            self.persist_session();
            self.last_save = instant;
        }
        self.check_goal_notification();
    }

    fn roll_session(&mut self, now: DateTime<Local>) -> Result<(), String> {
        self.persist_session();
        self.db
            .execute(
                "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
                params![timestamp(&now), self.session_id],
            )
            .map_err(|error| format!("Не удалось закрыть сессию: {error}"))?;
        let (session_id, active_seconds) = start_or_resume_day(&self.db, &now)
            .map_err(|error| format!("Не удалось начать новый день: {error}"))?;
        self.session_id = session_id;
        self.session_day = now.date_naive();
        self.session_started = now;
        self.active_seconds = active_seconds;
        self.session_active_seconds = 0;
        Ok(())
    }

    fn persist_session(&mut self) {
        if let Err(error) = self.db.execute(
            "UPDATE sessions SET active_seconds = ?1, updated_at = ?2 WHERE id = ?3",
            params![
                self.active_seconds as i64,
                timestamp(&Local::now()),
                self.session_id
            ],
        ) {
            self.db_error = Some(format!("Не удалось сохранить время: {error}"));
        }
    }

    fn close(&mut self) {
        self.persist_session();
        if let Err(error) = self.db.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2",
            params![timestamp(&Local::now()), self.session_id],
        ) {
            self.db_error = Some(format!("Не удалось закрыть сессию: {error}"));
        }
    }

    fn today_seconds(&self) -> u64 {
        self.active_seconds
    }

    fn check_goal_notification(&mut self) {
        let day_key = self.session_day.format("%Y-%m-%d").to_string();
        if !self.notify_goal
            || self.today_seconds() < u64::from(self.goal_hours) * 3600
            || self.goal_notified_day.as_deref() == Some(day_key.as_str())
        {
            return;
        }
        self.goal_notified_day = Some(day_key.clone());
        if let Err(error) = save_setting(&self.db, "goal_notified_day", &day_key) {
            self.db_error = Some(format!(
                "Не удалось сохранить состояние уведомления: {error}"
            ));
        }
        match Toast::new(Toast::POWERSHELL_APP_ID)
            .title("РИТМ")
            .text1("Цель активности достигнута")
            .text2(&format!("{} ч активного времени", self.goal_hours))
            .show()
        {
            Ok(()) => self.status_message = Some("Цель активности на сегодня выполнена".to_owned()),
            Err(error) => {
                self.status_message = Some(format!("Не удалось показать уведомление: {error}"))
            }
        }
    }

    fn snapshot(&self) -> Result<Snapshot, String> {
        let history = load_history(&self.db, 250)
            .map_err(|error| format!("Не удалось прочитать историю: {error}"))?;
        let week = load_week_activity(&self.db, self.session_day, self.today_seconds());
        Ok(Snapshot {
            today: self.session_day.format("%Y-%m-%d").to_string(),
            started_at: timestamp(&self.session_started),
            active_seconds: self.session_active_seconds,
            today_seconds: self.today_seconds(),
            idle_seconds: self.idle_seconds,
            active_now: self.active_now,
            goal_hours: self.goal_hours,
            goal_seconds: u64::from(self.goal_hours) * 3600,
            notify_goal: self.notify_goal,
            pause_minutes: self.pause_minutes,
            layout_type: self.layout_type.clone(),
            theme: self.theme.clone(),
            accent_index: self.accent_index,
            accent_color: ACCENTS[self.accent_index].to_owned(),
            autostart_enabled: self.autostart_enabled,
            history,
            week,
            db_error: self.db_error.clone(),
            status_message: self.status_message.clone(),
        })
    }

    fn set_preference(&mut self, key: &str, value: &str) -> Result<(), String> {
        let value_to_save = match key {
            "goal_hours" => {
                let number = value
                    .parse::<u16>()
                    .map_err(|_| "Неверная цель активности".to_owned())?
                    .clamp(1, 24);
                self.goal_hours = number;
                number.to_string()
            }
            "pause_minutes" => {
                let number = value
                    .parse::<u8>()
                    .map_err(|_| "Неверное время паузы".to_owned())?
                    .min(60);
                self.pause_minutes = number;
                number.to_string()
            }
            "layout_type" if ["quiet", "bento", "heatmap"].contains(&value) => {
                self.layout_type = value.to_owned();
                value.to_owned()
            }
            "theme" if ["light", "dark"].contains(&value) => {
                self.theme = value.to_owned();
                value.to_owned()
            }
            "accent_index" => {
                let number = value
                    .parse::<usize>()
                    .map_err(|_| "Неверный цвет".to_owned())?
                    .min(ACCENTS.len() - 1);
                self.accent_index = number;
                number.to_string()
            }
            "notify_goal" => {
                self.notify_goal =
                    parse_bool(value).ok_or_else(|| "Неверное значение уведомлений".to_owned())?;
                bool_setting(self.notify_goal).to_owned()
            }
            "autostart" => {
                let enabled =
                    parse_bool(value).ok_or_else(|| "Неверное значение автозапуска".to_owned())?;
                set_autostart(enabled)
                    .map_err(|error| format!("Не удалось изменить автозапуск: {error}"))?;
                self.autostart_enabled = enabled;
                bool_setting(enabled).to_owned()
            }
            _ => return Err("Неизвестная настройка".to_owned()),
        };
        save_setting(&self.db, key, &value_to_save)
            .map_err(|error| format!("Не удалось сохранить настройку: {error}"))?;
        self.db_error = None;
        self.status_message = None;
        Ok(())
    }
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, SharedTracker>) -> Result<Snapshot, String> {
    state
        .lock()
        .map_err(|_| "Трекер занят".to_owned())?
        .snapshot()
}

#[tauri::command]
pub fn set_preference(
    state: State<'_, SharedTracker>,
    key: String,
    value: String,
) -> Result<(), String> {
    state
        .lock()
        .map_err(|_| "Трекер занят".to_owned())?
        .set_preference(&key, &value)
}

pub fn start_worker(state: SharedTracker) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            if let Ok(mut tracker) = state.lock() {
                tracker.tick();
            }
        }
    });
}

pub fn close_session(state: &SharedTracker) {
    if let Ok(mut tracker) = state.lock() {
        tracker.close();
    }
}

pub fn attach_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть Ритм", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;
    TrayIconBuilder::new()
        .icon(create_tray_icon())
        .menu(&menu)
        .tooltip("Ритм — учёт активности")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "quit" => app.exit(0),
            _ => (),
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn handle_window_event(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

fn create_tray_icon() -> tauri::image::Image<'static> {
    let size = 32u32;
    let mut rgba = vec![0; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let radius = (dx * dx + dy * dy).sqrt();
            if radius <= 14.0 {
                let ring = (12.0..=14.0).contains(&radius);
                let hand =
                    (x == 15 && (8..=17).contains(&y)) || (y == 17 && (15..=23).contains(&x));
                let color = if ring || hand {
                    (82, 133, 91)
                } else {
                    (22, 36, 44)
                };
                let offset = ((y * size + x) * 4) as usize;
                rgba[offset..offset + 4].copy_from_slice(&[color.0, color.1, color.2, 255]);
            }
        }
    }
    tauri::image::Image::new_owned(rgba, size, size)
}

fn database_path() -> Result<PathBuf, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("Не удалось определить путь приложения: {error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| "Не удалось определить папку приложения.".to_owned())?;
    Ok(directory.join("activity-check-database").join("activity.sqlite3"))
}

fn start_or_resume_day(db: &Connection, now: &DateTime<Local>) -> rusqlite::Result<(i64, u64)> {
    let day = now.date_naive().format("%Y-%m-%d").to_string();
    let now_text = timestamp(now);
    let existing = db
        .query_row(
            "SELECT id, active_seconds FROM sessions WHERE day = ?1",
            [&day],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((id, seconds)) = existing {
        db.execute(
            "UPDATE sessions SET updated_at = ?1, ended_at = NULL WHERE id = ?2",
            params![now_text, id],
        )?;
        return Ok((id, seconds.max(0) as u64));
    }

    db.execute(
        "INSERT INTO sessions(day, started_at, updated_at, active_seconds) VALUES (?1, ?2, ?2, 0)",
        params![day, now_text],
    )?;
    Ok((db.last_insert_rowid(), 0))
}

fn load_history(db: &Connection, limit: usize) -> rusqlite::Result<Vec<SessionRow>> {
    let mut statement = db.prepare(
        "SELECT day, started_at, active_seconds, ended_at FROM sessions ORDER BY day DESC LIMIT ?1",
    )?;
    let rows = statement.query_map([limit as i64], |row| {
        Ok(SessionRow {
            day: row.get(0)?,
            started_at: row.get(1)?,
            active_seconds: row.get::<_, i64>(2)?.max(0) as u64,
            ended_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

fn ensure_updated_at_column(db: &Connection) -> rusqlite::Result<()> {
    let mut statement = db.prepare("PRAGMA table_info(sessions)")?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut found = false;
    for column in columns {
        if column? == "updated_at" {
            found = true;
            break;
        }
    }
    drop(statement);
    if !found {
        db.execute(
            "ALTER TABLE sessions ADD COLUMN updated_at TEXT NOT NULL DEFAULT ''",
            [],
        )?;
        db.execute("UPDATE sessions SET updated_at = COALESCE(NULLIF(ended_at, ''), started_at) WHERE updated_at = ''", [])?;
    }
    Ok(())
}

fn merge_sessions_by_day(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "BEGIN IMMEDIATE;
         DROP TABLE IF EXISTS sessions_by_day_migration;
         CREATE TABLE sessions_by_day_migration (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             day TEXT NOT NULL UNIQUE,
             started_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             ended_at TEXT,
             active_seconds INTEGER NOT NULL DEFAULT 0
         );
         INSERT INTO sessions_by_day_migration(day, started_at, updated_at, ended_at, active_seconds)
         SELECT day, MIN(started_at), MAX(updated_at),
                CASE WHEN COUNT(ended_at) < COUNT(*) THEN NULL ELSE MAX(ended_at) END,
                SUM(active_seconds)
           FROM sessions
          GROUP BY day;
         DROP TABLE sessions;
         ALTER TABLE sessions_by_day_migration RENAME TO sessions;
         CREATE INDEX idx_sessions_day ON sessions(day);
         COMMIT;",
    )?;
    Ok(())
}

fn load_week_activity(db: &Connection, today: NaiveDate, today_total: u64) -> Vec<WeekDay> {
    let start = today - chrono::Duration::days(6);
    let mut values = HashMap::new();
    if let Ok(mut statement) = db.prepare("SELECT day, COALESCE(SUM(active_seconds), 0) FROM sessions WHERE day >= ?1 AND day <= ?2 GROUP BY day") {
        if let Ok(rows) = statement.query_map(params![start.format("%Y-%m-%d").to_string(), today.format("%Y-%m-%d").to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))) {
            for (day, seconds) in rows.flatten() { values.insert(day, seconds.max(0) as u64); }
        }
    }
    (0..7)
        .map(|offset| {
            let date = start + chrono::Duration::days(offset);
            let key = date.format("%Y-%m-%d").to_string();
            let seconds = if date == today {
                today_total
            } else {
                values.get(&key).copied().unwrap_or(0)
            };
            WeekDay {
                date: key,
                label: date.format("%a").to_string(),
                seconds,
            }
        })
        .collect()
}

fn load_setting(db: &Connection, key: &str, default: &str) -> Result<String, String> {
    db.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
    .map(|value: Option<String>| value.unwrap_or_else(|| default.to_owned()))
    .map_err(|error| format!("Не удалось прочитать настройку {key}: {error}"))
}

fn save_setting(db: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    db.execute("INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
    Ok(())
}

fn timestamp(date_time: &DateTime<Local>) -> String {
    date_time.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn idle_seconds() -> Option<u64> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // Windows exposes only the timestamp of the last input, never its contents.
    let succeeded = unsafe { GetLastInputInfo(&mut info) };
    if succeeded == 0 {
        return None;
    }
    let elapsed_ms = unsafe { GetTickCount() }.wrapping_sub(info.dwTime);
    Some(u64::from(elapsed_ms) / 1000)
}

fn set_autostart(enabled: bool) -> std::io::Result<()> {
    if enabled {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
        let command = format!("\"{}\"", env::current_exe()?.display());
        key.set_value(RUN_VALUE, &command)
    } else {
        let key = match RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_SET_VALUE)
        {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        match key.delete_value(RUN_VALUE) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}
fn bool_setting(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}
