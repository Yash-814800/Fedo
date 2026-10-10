// Calendar access (FEDO.md Phase 5). GNOME Calendar keeps its events in
// Evolution Data Server; the only supported way in is its GObject API, so a
// small Python helper (helpers/fedo-calendar.py, embedded here) does the work
// and answers in JSON. Also the local-time helpers the scheduler and the
// router share — std has no time zones.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

const HELPER: &str = include_str!("../helpers/fedo-calendar.py");

// ── Local time ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    pub month: u32, // 1–12
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub weekday: u32, // 0 = Sunday
}

pub fn local(epoch: i64) -> Local {
    let t = epoch as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    Local {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        weekday: tm.tm_wday as u32,
    }
}

/// Epoch of a local wall-clock time. Day/hour overflow is normalised
/// (`day = 0` is the last day of the previous month), DST decided by the zone.
pub fn from_local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> i64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour;
    tm.tm_min = minute;
    tm.tm_isdst = -1;
    unsafe { libc::mktime(&mut tm) as i64 }
}

/// Local midnight at the start of `epoch`'s day, plus `days`.
pub fn day_start(epoch: i64, days: i32) -> i64 {
    let l = local(epoch);
    from_local(l.year, l.month as i32, l.day as i32 + days, 0, 0)
}

/// "YYYY-MM-DDTHH:MM" or "YYYY-MM-DD" (local) → epoch; `all_day` for the latter.
pub fn parse_local(s: &str) -> Option<(i64, bool)> {
    let s = s.trim();
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut d = date.split('-');
    let (y, m, day): (i32, i32, i32) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    if !(1970..=2200).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    match time {
        None => Some((from_local(y, m, day, 12, 0), true)),
        Some(t) => {
            let mut p = t.split(':');
            let (h, mi): (i32, i32) = (p.next()?.parse().ok()?, p.next().unwrap_or("0").parse().ok()?);
            if !(0..24).contains(&h) || !(0..60).contains(&mi) {
                return None;
            }
            Some((from_local(y, m, day, h, mi), false))
        }
    }
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "Fri 16 Oct, 4:00 pm" / "Tue 20 Oct" for all-day.
pub fn describe(epoch: i64, all_day: bool) -> String {
    let l = local(epoch);
    let date = format!("{} {} {}", DAYS[l.weekday as usize], l.day, MONTHS[(l.month - 1) as usize]);
    if all_day {
        return date;
    }
    let (h12, ampm) = match l.hour {
        0 => (12, "am"),
        1..=11 => (l.hour, "am"),
        12 => (12, "pm"),
        h => (h - 12, "pm"),
    };
    format!("{date}, {h12}:{:02} {ampm}", l.minute)
}

/// For the model's prompt: "Saturday 2026-10-10 18:52".
pub fn now_for_prompt(epoch: i64) -> String {
    let l = local(epoch);
    const FULL: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    format!(
        "{} {:04}-{:02}-{:02} {:02}:{:02}",
        FULL[l.weekday as usize], l.year, l.month, l.day, l.hour, l.minute
    )
}

/// The next two weeks as a lookup table for a model that cannot do calendar
/// arithmetic: "Friday = 2026-10-16". The first of each weekday is what a bare
/// weekday or "next <weekday>" means.
pub fn days_ahead_for_prompt(now: i64, days: i32) -> String {
    const FULL: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let mut parts = Vec::new();
    for i in 0..days {
        let t = day_start(now, i) + 12 * 3600;
        let l = local(t);
        let label = match i {
            0 => format!("today {}", FULL[l.weekday as usize]),
            1 => format!("tomorrow {}", FULL[l.weekday as usize]),
            _ => FULL[l.weekday as usize].to_string(),
        };
        parts.push(format!("{label} = {:04}-{:02}-{:02}", l.year, l.month, l.day));
    }
    parts.join("; ")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── The helper ────────────────────────────────────────────────────────────────

fn call(request: Value) -> Result<Value, String> {
    let mut child = Command::new("python3")
        .args(["-c", HELPER, &request.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "python3 isn't available, so I can't reach your calendar.".to_string())?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let mut stderr = child.stderr.take().ok_or("no output")?;
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The calendar didn't answer in time.".into());
            }
        }
    }
    let out = out.join().unwrap_or_default();
    let v: Value = serde_json::from_str(&out).map_err(|_| {
        let e = err.join().unwrap_or_default();
        let last = e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("calendar helper failed");
        format!("Calendar helper: {last}")
    })?;
    if v["ok"].as_bool() == Some(true) {
        Ok(v)
    } else {
        Err(v["error"].as_str().unwrap_or("calendar error").to_string())
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalEvent {
    pub uid: String,
    pub title: String,
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub all_day: bool,
    pub calendar: String,
}

/// Creates an event in the default calendar; returns (uid, calendar name).
pub fn create_event(
    title: &str,
    start: i64,
    end: i64,
    all_day: bool,
    alarms: &[i64],
    description: &str,
) -> Result<(String, String), String> {
    let v = call(json!({
        "op": "create", "title": title, "start": start, "end": end,
        "allDay": all_day, "alarms": alarms, "description": description,
    }))?;
    Ok((
        v["uid"].as_str().unwrap_or_default().to_string(),
        v["calendar"].as_str().unwrap_or("calendar").to_string(),
    ))
}

/// The name of the calendar new events go to.
pub fn default_calendar_name() -> Result<String, String> {
    let v = call(json!({ "op": "calendars" }))?;
    v["calendars"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["default"].as_bool() == Some(true)))
        .and_then(|c| c["name"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "no default calendar".to_string())
}

pub fn events_between(start: i64, end: i64) -> Result<Vec<CalEvent>, String> {
    let v = call(json!({ "op": "range", "start": start, "end": end }))?;
    serde_json::from_value(v["events"].clone()).map_err(|e| e.to_string())
}

pub fn delete_event(uid: &str) -> Result<(), String> {
    call(json!({ "op": "delete", "uid": uid })).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_round_trip_and_normalise() {
        let t = from_local(2026, 10, 16, 16, 0);
        let l = local(t);
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (2026, 10, 16, 16, 0));
        assert_eq!(l.weekday, 5); // Friday
        // day 0 is the last day of the previous month; -3 days is plain arithmetic
        let d = local(from_local(2026, 11, 0, 9, 0));
        assert_eq!((d.month, d.day), (10, 31));
        let before = local(from_local(2026, 10, 20 - 3, 9, 0));
        assert_eq!(before.day, 17);
    }

    #[test]
    fn model_dates_are_read_and_bad_ones_refused() {
        let (t, all) = parse_local("2026-10-16T16:00").unwrap();
        assert!(!all);
        assert_eq!(local(t).hour, 16);
        let (d, all) = parse_local("2026-10-20").unwrap();
        assert!(all);
        assert_eq!(local(d).day, 20);
        assert!(parse_local("tomorrow").is_none());
        assert!(parse_local("2026-13-40").is_none());
        assert!(parse_local("2026-10-16T25:00").is_none());
    }

    #[test]
    fn times_read_the_way_people_say_them() {
        let t = from_local(2026, 10, 16, 16, 5);
        assert_eq!(describe(t, false), "Fri 16 Oct, 4:05 pm");
        assert_eq!(describe(from_local(2026, 10, 20, 12, 0), true), "Tue 20 Oct");
        assert_eq!(describe(from_local(2026, 10, 16, 0, 30), false), "Fri 16 Oct, 12:30 am");
    }

    #[test]
    fn the_calendar_table_names_each_day() {
        let t = from_local(2026, 10, 10, 21, 0);
        let s = days_ahead_for_prompt(t, 8);
        assert!(s.starts_with("today Saturday = 2026-10-10; tomorrow Sunday = 2026-10-11; Monday = 2026-10-12"), "{s}");
        assert!(s.contains("Friday = 2026-10-16"));
        assert!(s.contains("Saturday = 2026-10-17"));
    }

    #[test]
    fn day_start_is_local_midnight() {
        let t = from_local(2026, 10, 16, 16, 5);
        let s = day_start(t, 0);
        assert_eq!((local(s).hour, local(s).minute, local(s).day), (0, 0, 16));
        assert_eq!(local(day_start(t, 1)).day, 17);
    }

    /// Against the real Evolution Data Server: creates one event in the default
    /// calendar, reads it back and deletes it.
    /// `cargo test -p fedo --lib calendar -- --ignored`
    #[test]
    #[ignore]
    fn live_eds_round_trip() {
        let start = now() + 5 * 86400;
        let (uid, cal) = create_event("Fedo round-trip test", start, start + 1800, false, &[-900], "").unwrap();
        assert!(!uid.is_empty(), "no uid ({cal})");
        let found = events_between(start - 60, start + 3600).unwrap();
        assert!(found.iter().any(|e| e.uid == uid && e.title == "Fedo round-trip test"));
        delete_event(&uid).unwrap();
        let after = events_between(start - 60, start + 3600).unwrap();
        assert!(!after.iter().any(|e| e.uid == uid));
    }
}
