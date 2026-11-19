//! Timers and reminders: a small schedule persisted at
//! `~/.boris/state/reminders.json`, shared by the agent tools (`timer_set`,
//! `reminder_set`, `reminders_list`, `reminder_cancel`) and the voice host
//! that speaks each item when it is due.
//!
//! One process-wide [`ReminderStore`] exists per file, so the tools (agent
//! thread) and the host (engine thread) share one lock and one in-memory copy.
//! Every change is written atomically (temp file + rename) and the file is
//! read back on open, so items survive restarts. An item is removed only when
//! the host takes it to speak it ([`ReminderStore::take_due`]); until then it
//! stays due, so a busy host simply delivers it later.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

/// Most items kept at once.
pub const MAX_REMINDERS: usize = 100;
/// Longest message stored.
pub const MAX_REMINDER_MESSAGE_CHARS: usize = 300;
/// Furthest ahead an item may be set.
pub const MAX_REMINDER_HORIZON_DAYS: i64 = 366;
/// Overdue by more than this when spoken: the line says when it was due.
const LATE_AFTER_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    Timer,
    Reminder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: u64,
    pub kind: ReminderKind,
    pub message: String,
    /// Due time, Unix milliseconds.
    pub due_unix_ms: i64,
    pub created_unix_ms: i64,
}

impl Reminder {
    pub fn due_local(&self) -> DateTime<Local> {
        Local
            .timestamp_millis_opt(self.due_unix_ms)
            .single()
            .unwrap_or_else(Local::now)
    }

    /// What the host says when this item fires at `now_ms`.
    pub fn spoken(&self, now_ms: i64) -> String {
        let late = now_ms.saturating_sub(self.due_unix_ms) > LATE_AFTER_MS;
        let message = self.message.trim().trim_end_matches(['.', '!']);
        let when = self.due_local().format("%-I:%M %p");
        match (self.kind, late, message.is_empty()) {
            (ReminderKind::Timer, false, true) => "Your timer is done.".into(),
            (ReminderKind::Timer, false, false) => format!("Your timer is done: {message}."),
            (ReminderKind::Timer, true, _) if message.is_empty() => {
                format!("Your timer for {when} went off while I was away.")
            }
            (ReminderKind::Timer, true, _) => format!("Timer from {when}: {message}."),
            (ReminderKind::Reminder, false, _) => format!("Reminder: {message}."),
            (ReminderKind::Reminder, true, _) => format!("Reminder from {when}: {message}."),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ReminderFile {
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    items: Vec<Reminder>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<ReminderFile>,
}

/// Shared handle to one reminders file.
#[derive(Clone)]
pub struct ReminderStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ReminderStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReminderStore")
            .field("path", &self.inner.path)
            .finish()
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Arc<Inner>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<Inner>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Current time in Unix milliseconds (the unit of [`Reminder::due_unix_ms`]).
pub fn now_unix_ms() -> i64 {
    Local::now().timestamp_millis()
}

impl ReminderStore {
    /// `<boris_home>/state/reminders.json`.
    pub fn path_for_home(boris_home: &Path) -> PathBuf {
        boris_home.join("state").join("reminders.json")
    }

    pub fn for_home(boris_home: &Path) -> Self {
        Self::open(Self::path_for_home(boris_home))
    }

    /// The process-wide store for `path`, loaded from disk on first use.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut map = registry().lock().unwrap_or_else(|e| e.into_inner());
        let inner = map
            .entry(path.clone())
            .or_insert_with(|| {
                Arc::new(Inner {
                    state: Mutex::new(load_file(&path)),
                    path,
                })
            })
            .clone();
        Self { inner }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut ReminderFile) -> T) -> T {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut state)
    }

    /// Schedule a new item. Fails on an empty reminder message, a time in
    /// the past or too far ahead, a full list, or a write error.
    pub fn add(
        &self,
        kind: ReminderKind,
        message: &str,
        due: DateTime<Local>,
    ) -> Result<Reminder, String> {
        let now = Local::now();
        if due <= now {
            return Err("that time has already passed".into());
        }
        if due - now > Duration::days(MAX_REMINDER_HORIZON_DAYS) {
            return Err(format!(
                "reminders can be at most {MAX_REMINDER_HORIZON_DAYS} days ahead"
            ));
        }
        let message: String = message
            .trim()
            .chars()
            .take(MAX_REMINDER_MESSAGE_CHARS)
            .collect();
        if kind == ReminderKind::Reminder && message.is_empty() {
            return Err("a reminder needs a message".into());
        }
        self.with_state(|state| {
            if state.items.len() >= MAX_REMINDERS {
                return Err(format!(
                    "already {MAX_REMINDERS} timers and reminders; cancel some first"
                ));
            }
            state.next_id = state.next_id.max(1);
            let item = Reminder {
                id: state.next_id,
                kind,
                message,
                due_unix_ms: due.timestamp_millis(),
                created_unix_ms: now.timestamp_millis(),
            };
            let mut next = state.clone();
            next.next_id += 1;
            next.items.push(item.clone());
            save_file(&self.inner.path, &next)?;
            *state = next;
            Ok(item)
        })
    }

    /// All items, soonest first.
    pub fn list(&self) -> Vec<Reminder> {
        let mut items = self.with_state(|state| state.items.clone());
        sort_due(&mut items);
        items
    }

    /// Remove items by id. Returns the removed ones.
    pub fn cancel(&self, ids: &[u64]) -> Result<Vec<Reminder>, String> {
        self.remove_where(|item| ids.contains(&item.id))
    }

    /// Remove items whose message contains `text` (case-insensitive).
    pub fn cancel_matching(&self, text: &str) -> Result<Vec<Reminder>, String> {
        let needle = text.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        self.remove_where(|item| item.message.to_lowercase().contains(&needle))
    }

    /// Soonest due time, if any.
    pub fn next_due_ms(&self) -> Option<i64> {
        self.with_state(|state| state.items.iter().map(|i| i.due_unix_ms).min())
    }

    pub fn has_due(&self, now_ms: i64) -> bool {
        self.next_due_ms().is_some_and(|due| due <= now_ms)
    }

    /// Remove and return every item due at `now_ms`, earliest first. If the
    /// file cannot be written the items are still returned (spoken once) and
    /// kept out of memory, so a broken disk cannot make one repeat forever.
    pub fn take_due(&self, now_ms: i64) -> Vec<Reminder> {
        let mut due = self.with_state(|state| {
            let (due, rest): (Vec<_>, Vec<_>) = state
                .items
                .drain(..)
                .partition(|item| item.due_unix_ms <= now_ms);
            state.items = rest;
            if !due.is_empty() {
                if let Err(error) = save_file(&self.inner.path, state) {
                    tracing::warn!(%error, "reminders: could not persist delivery");
                }
            }
            due
        });
        sort_due(&mut due);
        due
    }

    fn remove_where(&self, pred: impl Fn(&Reminder) -> bool) -> Result<Vec<Reminder>, String> {
        self.with_state(|state| {
            let (gone, kept): (Vec<_>, Vec<_>) =
                state.items.iter().cloned().partition(|item| pred(item));
            if gone.is_empty() {
                return Ok(gone);
            }
            let next = ReminderFile {
                next_id: state.next_id,
                items: kept,
            };
            save_file(&self.inner.path, &next)?;
            *state = next;
            Ok(gone)
        })
    }
}

fn sort_due(items: &mut [Reminder]) {
    items.sort_by_key(|item| (item.due_unix_ms, item.id));
}

fn load_file(path: &Path) -> ReminderFile {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "reminders file unreadable; starting empty");
            ReminderFile::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReminderFile::default(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "reminders file unreadable; starting empty");
            ReminderFile::default()
        }
    }
}

fn save_file(path: &Path, file: &ReminderFile) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("save {}: {e}", path.display()))
}

/// Human label for a duration ("1 hour 30 minutes").
pub fn describe_duration(d: Duration) -> String {
    let total = d.num_seconds().max(0);
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    let mut parts = Vec::new();
    let mut push = |n: i64, unit: &str| {
        if n > 0 {
            parts.push(format!("{n} {unit}{}", if n == 1 { "" } else { "s" }));
        }
    };
    push(h, "hour");
    push(m, "minute");
    push(s, "second");
    if parts.is_empty() {
        "0 seconds".into()
    } else {
        parts.join(" ")
    }
}

const DURATION_HELP: &str = "use a duration like 10m, 90s, 1h30m, or \"2 hours\"";
const WHEN_HELP: &str = "use a duration (\"in 20 minutes\", \"1h\"), a time of day (\"5pm\", \"17:30\", \"tomorrow 9am\"), or a date and time (\"2026-11-20 09:00\")";

/// Parse a duration: `90s`, `10m`, `1h30m`, `1.5h`, `2 hours 5 minutes`,
/// optionally after "in" / "for". A bare number means minutes.
pub fn parse_duration(input: &str) -> Result<Duration, String> {
    let text = input.trim().to_lowercase();
    let text = text
        .strip_prefix("in ")
        .or_else(|| text.strip_prefix("for "))
        .unwrap_or(&text)
        .trim();
    if text.is_empty() {
        return Err(format!("missing duration; {DURATION_HELP}"));
    }
    if let Ok(minutes) = text.parse::<f64>() {
        return checked_secs(minutes * 60.0);
    }
    let mut total = 0f64;
    let mut seen = false;
    let mut rest = text;
    while !rest.is_empty() {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        rest = rest.strip_prefix("and ").unwrap_or(rest);
        if rest.is_empty() {
            break;
        }
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (number, after) = if num_end == 0 {
            // "an hour", "a minute", "half an hour"
            if let Some(after) = rest
                .strip_prefix("half an ")
                .or(rest.strip_prefix("half a "))
            {
                (0.5, after)
            } else if let Some(after) = rest.strip_prefix("an ").or(rest.strip_prefix("a ")) {
                (1.0, after)
            } else {
                return Err(format!("could not read \"{input}\"; {DURATION_HELP}"));
            }
        } else {
            let number: f64 = rest[..num_end]
                .parse()
                .map_err(|_| format!("could not read \"{input}\"; {DURATION_HELP}"))?;
            (number, &rest[num_end..])
        };
        let after = after.trim_start();
        let unit_end = after
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(after.len());
        let unit = &after[..unit_end];
        let scale = match unit {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "d" | "day" | "days" => 86_400.0,
            _ => return Err(format!("could not read \"{input}\"; {DURATION_HELP}")),
        };
        total += number * scale;
        seen = true;
        rest = &after[unit_end..];
    }
    if !seen {
        return Err(format!("could not read \"{input}\"; {DURATION_HELP}"));
    }
    checked_secs(total)
}

fn checked_secs(secs: f64) -> Result<Duration, String> {
    if !secs.is_finite() || secs < 1.0 {
        return Err("the duration must be at least one second".into());
    }
    if secs > (MAX_REMINDER_HORIZON_DAYS * 86_400) as f64 {
        return Err(format!(
            "the duration can be at most {MAX_REMINDER_HORIZON_DAYS} days"
        ));
    }
    Ok(Duration::milliseconds((secs * 1000.0).round() as i64))
}

/// Parse when a reminder is due, relative to `now`: a duration ("in 20
/// minutes"), a time of day ("5pm", "17:30", "noon"; the next one if it
/// already passed today), "today"/"tonight"/"tomorrow" plus a time, a date
/// and time ("2026-11-20 09:00"), or RFC 3339.
pub fn parse_when(input: &str, now: DateTime<Local>) -> Result<DateTime<Local>, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(format!("missing time; {WHEN_HELP}"));
    }
    if let Ok(at) = DateTime::parse_from_rfc3339(raw) {
        return Ok(at.with_timezone(&Local));
    }
    for fmt in [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            return local(naive);
        }
    }
    let lower = raw.to_lowercase();
    let at_prefix = lower.starts_with("at ");
    let text = lower.strip_prefix("at ").unwrap_or(&lower).trim();
    // "at 9" is a time, never nine minutes; a bare number is ambiguous here.
    let bare_number = text.parse::<f64>().is_ok();
    if !at_prefix && (text.starts_with("in ") || (!bare_number && parse_duration(text).is_ok())) {
        return Ok(now + parse_duration(text)?);
    }

    let (day, rest, default_time) = if let Some(rest) = text.strip_prefix("tomorrow") {
        (
            Some(now.date_naive() + Duration::days(1)),
            rest,
            Some((9, 0)),
        )
    } else if let Some(rest) = text.strip_prefix("tonight") {
        (Some(now.date_naive()), rest, Some((20, 0)))
    } else if let Some(rest) = text.strip_prefix("today") {
        (Some(now.date_naive()), rest, None)
    } else {
        (None, text, None)
    };
    let rest = rest.trim();
    let rest = rest.strip_prefix("at ").unwrap_or(rest).trim();
    let time = if rest.is_empty() {
        match default_time {
            Some((h, m)) => NaiveTime::from_hms_opt(h, m, 0),
            None => None,
        }
        .ok_or_else(|| format!("missing time of day in \"{input}\"; {WHEN_HELP}"))?
    } else {
        parse_time_of_day(rest).ok_or_else(|| format!("could not read \"{input}\"; {WHEN_HELP}"))?
    };
    let date = day.unwrap_or_else(|| now.date_naive());
    let mut at = local(date.and_time(time))?;
    if day.is_none() && at <= now {
        at = local((date + Duration::days(1)).and_time(time))?;
    }
    if at <= now {
        return Err(format!("\"{input}\" has already passed"));
    }
    Ok(at)
}

fn local(naive: NaiveDateTime) -> Result<DateTime<Local>, String> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .ok_or_else(|| "that local time does not exist (clock change)".into())
}

/// "5pm", "5 pm", "5:30pm", "5:30 p.m.", "17:30", "noon", "midnight".
fn parse_time_of_day(text: &str) -> Option<NaiveTime> {
    let text = text.trim().replace('.', "");
    match text.as_str() {
        "noon" | "midday" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (body, meridiem) = if let Some(b) = text.strip_suffix("am") {
        (b.trim(), Some(false))
    } else if let Some(b) = text.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else {
        (text.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        // A bare hour needs am/pm ("at 9" is ambiguous).
        None if meridiem.is_some() => (body.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match meridiem {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            (h % 12) + if pm { 12 } else { 0 }
        }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        local(
            NaiveDate::from_ymd_opt(y, mo, d)
                .unwrap()
                .and_hms_opt(h, mi, 0)
                .unwrap(),
        )
        .unwrap()
    }

    fn temp_store(tag: &str) -> ReminderStore {
        let path = std::env::temp_dir().join(format!(
            "boris-reminders-{tag}-{}-{}.json",
            std::process::id(),
            now_unix_ms()
        ));
        let _ = std::fs::remove_file(&path);
        ReminderStore::open(path)
    }

    #[test]
    fn durations_parse_in_common_spoken_forms() {
        let secs = |s: &str| parse_duration(s).unwrap().num_seconds();
        assert_eq!(secs("90s"), 90);
        assert_eq!(secs("10m"), 600);
        assert_eq!(secs("10 minutes"), 600);
        assert_eq!(secs("in 2 hours"), 7200);
        assert_eq!(secs("1h30m"), 5400);
        assert_eq!(secs("1 hour and 5 minutes"), 3900);
        assert_eq!(secs("1.5h"), 5400);
        assert_eq!(secs("for 5"), 300, "bare number means minutes");
        assert_eq!(secs("half an hour"), 1800);
        assert_eq!(secs("an hour"), 3600);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("soon").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("10 fortnights").is_err());
    }

    #[test]
    fn times_parse_relative_absolute_and_next_occurrence() {
        let now = at(2026, 11, 19, 14, 0);
        assert_eq!(
            parse_when("in 20 minutes", now).unwrap(),
            at(2026, 11, 19, 14, 20)
        );
        assert_eq!(parse_when("1h", now).unwrap(), at(2026, 11, 19, 15, 0));
        assert_eq!(parse_when("5pm", now).unwrap(), at(2026, 11, 19, 17, 0));
        assert_eq!(
            parse_when("at 5:30 p.m.", now).unwrap(),
            at(2026, 11, 19, 17, 30)
        );
        assert_eq!(parse_when("17:45", now).unwrap(), at(2026, 11, 19, 17, 45));
        // Already passed today: the next one.
        assert_eq!(parse_when("9am", now).unwrap(), at(2026, 11, 20, 9, 0));
        assert_eq!(parse_when("noon", now).unwrap(), at(2026, 11, 20, 12, 0));
        assert_eq!(
            parse_when("tomorrow 7:15am", now).unwrap(),
            at(2026, 11, 20, 7, 15)
        );
        assert_eq!(parse_when("tomorrow", now).unwrap(), at(2026, 11, 20, 9, 0));
        assert_eq!(parse_when("tonight", now).unwrap(), at(2026, 11, 19, 20, 0));
        assert_eq!(
            parse_when("2026-11-21 08:05", now).unwrap(),
            at(2026, 11, 21, 8, 5)
        );
        assert!(
            parse_when("today 9am", now).is_err(),
            "explicit today in the past"
        );
        assert!(parse_when("at 9", now).is_err(), "bare hour is ambiguous");
        assert!(parse_when("9", now).is_err(), "bare number is ambiguous");
        assert!(parse_when("13pm", now).is_err());
        assert!(parse_when("whenever", now).is_err());
    }

    #[test]
    fn due_items_come_out_earliest_first_and_only_once() {
        let store = temp_store("due");
        let now = Local::now();
        let later = store
            .add(ReminderKind::Reminder, "later", now + Duration::seconds(30))
            .unwrap();
        let sooner = store
            .add(ReminderKind::Timer, "", now + Duration::seconds(10))
            .unwrap();
        let far = store
            .add(ReminderKind::Reminder, "far", now + Duration::hours(2))
            .unwrap();
        assert_eq!(
            store.list().iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![sooner.id, later.id, far.id]
        );
        let check = now.timestamp_millis() + 60_000;
        assert!(store.has_due(check));
        let due = store.take_due(check);
        assert_eq!(
            due.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![sooner.id, later.id]
        );
        assert!(store.take_due(check).is_empty(), "delivered once");
        assert_eq!(store.list(), vec![far.clone()]);

        // Survives a restart: the file holds exactly what is left.
        let reloaded = load_file(store.path());
        assert_eq!(reloaded.items, vec![far]);
        assert!(reloaded.next_id > sooner.id.max(later.id));
        let _ = std::fs::remove_file(store.path());
    }

    #[test]
    fn add_rejects_past_and_empty_and_cancel_matches() {
        let store = temp_store("cancel");
        let now = Local::now();
        assert!(store
            .add(ReminderKind::Reminder, "x", now - Duration::minutes(1))
            .is_err());
        assert!(store
            .add(ReminderKind::Reminder, "  ", now + Duration::minutes(1))
            .is_err());
        let a = store
            .add(
                ReminderKind::Reminder,
                "Call Mum",
                now + Duration::minutes(5),
            )
            .unwrap();
        store
            .add(
                ReminderKind::Reminder,
                "stretch",
                now + Duration::minutes(6),
            )
            .unwrap();
        assert_eq!(store.cancel_matching("call mum").unwrap(), vec![a.clone()]);
        assert!(store.cancel(&[a.id]).unwrap().is_empty());
        assert_eq!(store.list().len(), 1);
        let _ = std::fs::remove_file(store.path());
    }

    #[test]
    fn spoken_line_mentions_lateness() {
        let item = Reminder {
            id: 1,
            kind: ReminderKind::Reminder,
            message: "take the bread out".into(),
            due_unix_ms: 1_000_000,
            created_unix_ms: 0,
        };
        assert_eq!(item.spoken(1_000_500), "Reminder: take the bread out.");
        assert!(item
            .spoken(1_000_000 + 10 * 60_000)
            .starts_with("Reminder from "));
        let timer = Reminder {
            kind: ReminderKind::Timer,
            message: String::new(),
            ..item
        };
        assert_eq!(timer.spoken(1_000_500), "Your timer is done.");
        assert_eq!(
            describe_duration(Duration::seconds(5430)),
            "1 hour 30 minutes 30 seconds"
        );
    }
}
