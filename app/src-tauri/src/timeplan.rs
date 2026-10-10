// The time-shaped half of the assistant (FEDO.md Phase 5): "timer 25 min",
// "remind me in 10 minutes to check the oven", "Meeting with Riya Friday 4pm",
// "apply to Zomato before Oct 20", "what's on today?". The obvious phrasings are
// parsed here with no model; the rest come from the router as structured dates.
// Pure functions — what the scheduler and calendar then do is in their modules.

use serde::{Deserialize, Serialize};

use crate::calendar::{self, describe, from_local, local};

// ── Durations ─────────────────────────────────────────────────────────────────

fn unit_seconds(u: &str) -> Option<f64> {
    Some(match u {
        "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
        "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
        _ => return None,
    })
}

/// "25 min", "1 hour 30 minutes", "90s", "1.5 hours", "an hour", "half an hour",
/// or a bare "25" (minutes). The whole string must be a duration.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    let s = s.trim_end_matches(['.', '!', '?']).trim();
    match s {
        "an hour" | "one hour" | "a hour" => return Some(3600),
        "half an hour" | "half hour" | "half a hour" | "a half hour" => return Some(1800),
        "a minute" | "one minute" => return Some(60),
        "a quarter of an hour" | "a quarter hour" | "quarter of an hour" => return Some(900),
        _ => {}
    }
    // Glue "90s" / "25m" into "90 s" / "25 m", drop "and" and commas.
    let mut spaced = String::new();
    let mut prev_digit = false;
    for c in s.chars() {
        let digit = c.is_ascii_digit() || c == '.';
        if prev_digit && !digit && c != ' ' {
            spaced.push(' ');
        }
        spaced.push(if c == ',' { ' ' } else { c });
        prev_digit = digit;
    }
    let toks: Vec<&str> = spaced.split_whitespace().filter(|t| *t != "and").collect();
    if toks.is_empty() {
        return None;
    }
    if toks.len() == 1 {
        let n: f64 = toks[0].parse().ok()?;
        return secs(n * 60.0);
    }
    let mut total = 0.0;
    let mut i = 0;
    while i < toks.len() {
        let n: f64 = toks[i].parse().ok()?;
        let unit = unit_seconds(toks.get(i + 1)?)?;
        total += n * unit;
        i += 2;
    }
    secs(total)
}

fn secs(v: f64) -> Option<i64> {
    // A timer is at least a second and at most a day.
    (v >= 1.0 && v <= 86_400.0).then_some(v.round() as i64)
}

pub fn clock(seconds: i64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    match (h, m, s) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, 0, 0) => format!("{h} h"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

// ── Fast patterns ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Timed {
    Timer { seconds: i64 },
    /// Fires `seconds` from now with `title`.
    RemindIn { seconds: i64, title: String },
    Agenda { days: i32, label: &'static str },
    Deadlines,
}

pub fn fast(text: &str) -> Option<Timed> {
    let t = text.trim();
    let l = t.to_lowercase();
    let l = l.trim_end_matches(['.', '!', '?']).trim().to_string();

    // "timer 25 min" · "set a timer for 25 minutes" · "25 minute timer" · "set a 5 min timer"
    for p in ["set a timer for ", "set a timer ", "set timer for ", "set timer ", "start a timer for ", "start a timer ", "timer for ", "timer "] {
        if let Some(rest) = l.strip_prefix(p) {
            if let Some(s) = parse_duration(rest) {
                return Some(Timed::Timer { seconds: s });
            }
        }
    }
    if let Some(head) = l.strip_suffix(" timer") {
        let head = head.trim_start_matches("set a ").trim_start_matches("set an ").trim_start_matches("a ").trim();
        let head = head.replace("-minute", " minute").replace("-second", " second").replace("-hour", " hour");
        let head = head.replace(" minute", " minutes").replace(" minutess", " minutes");
        if let Some(s) = parse_duration(&head) {
            return Some(Timed::Timer { seconds: s });
        }
    }

    // "remind me in 10 minutes to check the oven" · "... that ..." · "remind me to check the oven in 10 minutes"
    if let Some(rest) = l.strip_prefix("remind me ") {
        let orig = &t[t.len() - rest.len()..];
        if let Some(r) = rest.strip_prefix("in ") {
            let o = &orig[3..];
            for sep in [" to ", " that ", " about ", ": "] {
                if let Some(i) = r.find(sep) {
                    if let Some(s) = parse_duration(&r[..i]) {
                        let title = o[i + sep.len()..].trim();
                        if !title.is_empty() {
                            return Some(Timed::RemindIn { seconds: s, title: title.to_string() });
                        }
                    }
                }
            }
        } else if let Some(r) = rest.strip_prefix("to ") {
            if let Some(i) = r.rfind(" in ") {
                if let Some(s) = parse_duration(&r[i + 4..]) {
                    let o = &orig[3..];
                    let title = o[..i].trim();
                    if !title.is_empty() {
                        return Some(Timed::RemindIn { seconds: s, title: title.to_string() });
                    }
                }
            }
        }
    }

    let asks_agenda = ["what's on", "whats on", "what is on", "what do i have", "what have i got", "my schedule", "my agenda", "my calendar", "agenda", "my day"]
        .iter()
        .any(|p| l.contains(p));
    if asks_agenda && l.split_whitespace().count() <= 8 {
        return Some(if l.contains("tomorrow") {
            Timed::Agenda { days: 1, label: "tomorrow" }
        } else if l.contains("this week") || l.contains("next 7") {
            Timed::Agenda { days: 7, label: "the next 7 days" }
        } else {
            Timed::Agenda { days: 0, label: "today" }
        });
    }
    if matches!(l.as_str(), "deadlines" | "my deadlines" | "show deadlines" | "show my deadlines" | "list deadlines" | "what are my deadlines") {
        return Some(Timed::Deadlines);
    }
    None
}

// ── Event and deadline cards ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderPlan {
    pub at: i64,
    pub label: String,
}

/// What the island shows before anything is written to the calendar.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCard {
    /// "event" or "deadline".
    pub kind: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    /// "Fri 16 Oct, 4:00 pm" / "Tue 20 Oct".
    pub when: String,
    /// Where it will go ("Personal").
    pub calendar: String,
    pub reminders: Vec<ReminderPlan>,
}

/// What the user configured for reminders.
#[derive(Debug, Clone)]
pub struct Prefs {
    pub event_minutes: u32,
    pub deadline_days: Vec<u32>,
    pub morning_hour: u32,
}

fn ago(days: u32) -> String {
    match days {
        0 => "the morning of".into(),
        1 => "1 day before".into(),
        d => format!("{d} days before"),
    }
}

/// When Fedo's own scheduler should speak up for an event or deadline. Times
/// already past are left out.
pub fn reminders(kind: &str, start: i64, all_day: bool, p: &Prefs, now: i64) -> Vec<ReminderPlan> {
    let mut out = Vec::new();
    if kind == "deadline" || all_day {
        let l = local(start);
        let mut days = p.deadline_days.clone();
        days.sort_unstable_by(|a, b| b.cmp(a));
        days.dedup();
        for d in days {
            let at = from_local(l.year, l.month as i32, l.day as i32 - d as i32, p.morning_hour as i32, 0);
            if at > now {
                out.push(ReminderPlan { at, label: ago(d) });
            }
        }
    } else if p.event_minutes > 0 {
        let at = start - p.event_minutes as i64 * 60;
        if at > now {
            out.push(ReminderPlan { at, label: format!("{} min before", p.event_minutes) });
        }
    }
    out
}

/// VALARM offsets (seconds from DTSTART) for the same plan, so Evolution's own
/// notifier agrees with Fedo's. Past ones are kept: harmless to the calendar.
pub fn alarm_offsets(kind: &str, start: i64, all_day: bool, p: &Prefs) -> Vec<i64> {
    if kind == "deadline" || all_day {
        let l = local(start);
        let midnight = from_local(l.year, l.month as i32, l.day as i32, 0, 0);
        p.deadline_days
            .iter()
            .map(|d| from_local(l.year, l.month as i32, l.day as i32 - *d as i32, p.morning_hour as i32, 0) - midnight)
            .collect()
    } else if p.event_minutes > 0 {
        vec![-(p.event_minutes as i64) * 60]
    } else {
        vec![]
    }
}

/// The notification text for a reminder about `title`.
pub fn reminder_text(kind: &str, title: &str, label: &str) -> String {
    match (kind, label) {
        ("deadline", "the morning of") => format!("Deadline today: {title}"),
        ("deadline", l) => format!("Deadline — {title} ({l})"),
        (_, l) => format!("{title} — {l}"),
    }
}

pub fn event_card(
    kind: &str,
    title: &str,
    start: i64,
    end: Option<i64>,
    all_day: bool,
    calendar: &str,
    p: &Prefs,
    now: i64,
) -> EventCard {
    let end = end.filter(|e| *e > start).unwrap_or(if all_day { start } else { start + 3600 });
    EventCard {
        kind: kind.to_string(),
        title: title.trim().chars().take(120).collect(),
        start,
        end,
        all_day,
        when: describe(start, all_day),
        calendar: calendar.to_string(),
        reminders: reminders(kind, start, all_day, p, now),
    }
}

/// "4:00 pm  Meeting with Riya" lines for an agenda.
pub fn agenda_text(label: &str, events: &[calendar::CalEvent]) -> String {
    if events.is_empty() {
        return format!("Nothing on your calendar for {label}.");
    }
    let mut out = format!("On your calendar for {label}:\n");
    for e in events.iter().take(15) {
        let when = match (e.start, e.all_day) {
            (Some(s), true) => format!("{} (all day)", describe(s, true)),
            (Some(s), false) => describe(s, false),
            _ => String::new(),
        };
        out.push_str(&format!("• {when} — {}\n", e.title));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_read_the_way_people_say_them() {
        assert_eq!(parse_duration("25 min"), Some(1500));
        assert_eq!(parse_duration("25"), Some(1500));
        assert_eq!(parse_duration("1 hour 30 minutes"), Some(5400));
        assert_eq!(parse_duration("1 hour and 30 minutes"), Some(5400));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("1.5 hours"), Some(5400));
        assert_eq!(parse_duration("half an hour"), Some(1800));
        assert_eq!(parse_duration("an hour"), Some(3600));
        assert_eq!(parse_duration("banana"), None);
        assert_eq!(parse_duration("0 min"), None);
        assert_eq!(parse_duration("48 hours"), None);
    }

    #[test]
    fn timers_and_reminders_are_caught_without_the_model() {
        assert_eq!(fast("timer 25 min"), Some(Timed::Timer { seconds: 1500 }));
        assert_eq!(fast("Set a timer for 10 minutes"), Some(Timed::Timer { seconds: 600 }));
        assert_eq!(fast("25 minute timer"), Some(Timed::Timer { seconds: 1500 }));
        assert_eq!(fast("set a 5-minute timer"), Some(Timed::Timer { seconds: 300 }));
        assert_eq!(
            fast("remind me in 10 minutes to check the oven"),
            Some(Timed::RemindIn { seconds: 600, title: "check the oven".into() })
        );
        assert_eq!(
            fast("Remind me to Call Mum in 2 hours"),
            Some(Timed::RemindIn { seconds: 7200, title: "Call Mum".into() })
        );
        assert_eq!(fast("remind me tomorrow to pay rent"), None);
        assert_eq!(fast("timer"), None);
    }

    #[test]
    fn agenda_and_deadline_questions() {
        assert_eq!(fast("what's on today?"), Some(Timed::Agenda { days: 0, label: "today" }));
        assert_eq!(fast("what do I have tomorrow"), Some(Timed::Agenda { days: 1, label: "tomorrow" }));
        assert_eq!(fast("what's on this week"), Some(Timed::Agenda { days: 7, label: "the next 7 days" }));
        assert_eq!(fast("deadlines"), Some(Timed::Deadlines));
        assert_eq!(fast("what's on my mind about the calendar widget bug in the island view"), None);
    }

    fn prefs() -> Prefs {
        Prefs { event_minutes: 15, deadline_days: vec![3, 1, 0], morning_hour: 9 }
    }

    #[test]
    fn a_deadline_gets_three_days_one_day_and_the_morning() {
        let now = from_local(2026, 10, 10, 18, 0);
        let due = from_local(2026, 10, 20, 12, 0);
        let r = reminders("deadline", due, true, &prefs(), now);
        let days: Vec<_> = r.iter().map(|x| (local(x.at).day, local(x.at).hour)).collect();
        assert_eq!(days, vec![(17, 9), (19, 9), (20, 9)]);
        assert_eq!(r[0].label, "3 days before");
        assert_eq!(r[2].label, "the morning of");
        // Past reminders are dropped: due in two days leaves only 1-day-before and the morning.
        let soon = from_local(2026, 10, 12, 12, 0);
        assert_eq!(reminders("deadline", soon, true, &prefs(), now).len(), 2);
        let offsets = alarm_offsets("deadline", due, true, &prefs());
        assert_eq!(offsets, vec![-3 * 86400 + 9 * 3600, -86400 + 9 * 3600, 9 * 3600]);
    }

    #[test]
    fn an_event_reminds_a_quarter_hour_before() {
        let now = from_local(2026, 10, 10, 18, 0);
        let start = from_local(2026, 10, 16, 16, 0);
        let r = reminders("event", start, false, &prefs(), now);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].at, start - 900);
        assert_eq!(alarm_offsets("event", start, false, &prefs()), vec![-900]);
        // Starting in five minutes: the reminder time has passed.
        assert!(reminders("event", now + 300, false, &prefs(), now).is_empty());
    }

    #[test]
    fn the_card_says_when_in_words() {
        let now = from_local(2026, 10, 10, 18, 0);
        let c = event_card("event", " Meeting with Riya ", from_local(2026, 10, 16, 16, 0), None, false, "Personal", &prefs(), now);
        assert_eq!(c.title, "Meeting with Riya");
        assert_eq!(c.when, "Fri 16 Oct, 4:00 pm");
        assert_eq!(c.end - c.start, 3600);
        assert_eq!(clock(5400), "1 h 30 min");
        assert_eq!(clock(45), "45 s");
        assert_eq!(reminder_text("deadline", "Zomato", "the morning of"), "Deadline today: Zomato");
    }
}
