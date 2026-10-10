// Timers, reminders and deadlines (FEDO.md Phase 5).
//
// One list of items, kept in ~/.local/share/fedo/schedule.json so it survives a
// restart. A single thread sleeps until the next item is due (Condvar with a
// timeout — no polling) and fires it: the island is told (waking it if it has
// been torn down), a GNOME notification goes out, and the item is removed.
// Items that fell due while Fedo was not running fire at start, marked missed.
//
// Kinds:
//   timer     counts down; can be paused (its remaining time is kept) and resumed
//   reminder  fires at a time; the scheduler's own, so it works even if
//             Evolution's alarm notifier isn't running
//   deadline  display only: the due date, for the deadlines list
//   event     display only: an event Fedo put in the calendar, for the list

use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::calendar::now;
use crate::{log, notify};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Timer,
    Reminder,
    Deadline,
    Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// When it fires (timer, reminder) or is due (deadline, event start), epoch s.
    pub fire_at: i64,
    /// A paused timer: seconds left. `fire_at` is meaningless while this is set.
    pub paused_left: Option<i64>,
    /// A timer's full length, for the progress ring.
    pub total: Option<i64>,
    pub all_day: bool,
    /// Calendar uid of the event this belongs to, if it was put in the calendar.
    pub event_uid: Option<String>,
    /// For a reminder: the deadline or event it is about.
    pub parent: Option<String>,
}

impl Default for Item {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: Kind::Reminder,
            title: String::new(),
            fire_at: 0,
            paused_left: None,
            total: None,
            all_day: false,
            event_uid: None,
            parent: None,
        }
    }
}

impl Item {
    fn fires(&self) -> bool {
        matches!(self.kind, Kind::Timer | Kind::Reminder) && self.paused_left.is_none()
    }
}

/// What the island is told when something fires.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fired {
    pub item: Item,
    /// It fell due while Fedo wasn't running.
    pub missed: bool,
}

// ── The list ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct List {
    pub items: Vec<Item>,
}

impl List {
    pub fn add(&mut self, mut item: Item) -> String {
        if item.id.is_empty() {
            item.id = format!("{:x}{:04x}", now(), self.items.len() as u16 ^ std::process::id() as u16);
            while self.items.iter().any(|i| i.id == item.id) {
                item.id.push('x');
            }
        }
        let id = item.id.clone();
        self.items.push(item);
        id
    }

    /// Items that are due at `at`, removed from the list and returned.
    pub fn take_due(&mut self, at: i64) -> Vec<Item> {
        let (due, keep): (Vec<Item>, Vec<Item>) =
            std::mem::take(&mut self.items).into_iter().partition(|i| i.fires() && i.fire_at <= at);
        self.items = keep;
        due
    }

    /// When the next timer or reminder is due.
    pub fn next_due(&self) -> Option<i64> {
        self.items.iter().filter(|i| i.fires()).map(|i| i.fire_at).min()
    }

    pub fn pause(&mut self, id: &str, at: i64) -> bool {
        match self.items.iter_mut().find(|i| i.id == id && i.kind == Kind::Timer && i.paused_left.is_none()) {
            Some(i) => {
                i.paused_left = Some((i.fire_at - at).max(1));
                true
            }
            None => false,
        }
    }

    pub fn resume(&mut self, id: &str, at: i64) -> bool {
        match self.items.iter_mut().find(|i| i.id == id && i.paused_left.is_some()) {
            Some(i) => {
                i.fire_at = at + i.paused_left.take().unwrap_or(0);
                true
            }
            None => false,
        }
    }

    /// Removes an item, and the reminders that were about it.
    pub fn cancel(&mut self, id: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|i| i.id != id && i.parent.as_deref() != Some(id));
        self.items.len() != before
    }

    /// Display-only items stay a day past their date, then go.
    pub fn prune(&mut self, at: i64) {
        self.items.retain(|i| !matches!(i.kind, Kind::Deadline | Kind::Event) || i.fire_at + 86_400 > at);
    }

    /// Sorted for showing: running timers by time left, then everything by date.
    pub fn sorted(&self) -> Vec<Item> {
        let mut v = self.items.clone();
        v.sort_by_key(|i| (i.kind != Kind::Timer, i.paused_left.map(|p| p + 0).unwrap_or(i.fire_at)));
        v
    }
}

// ── The service ───────────────────────────────────────────────────────────────

pub struct Scheduler {
    list: Mutex<List>,
    wake: Condvar,
    path: PathBuf,
}

fn load(path: &PathBuf) -> List {
    let items: Vec<Item> = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    List { items }
}

impl Scheduler {
    pub fn load() -> Self {
        let path = crate::platform::local_dir().join("schedule.json");
        let mut list = load(&path);
        list.prune(now());
        Scheduler { list: Mutex::new(list), wake: Condvar::new(), path }
    }

    fn save(&self, list: &List) {
        let tmp = self.path.with_extension("json.tmp");
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let ok = serde_json::to_vec_pretty(&list.items)
            .ok()
            .and_then(|b| std::fs::write(&tmp, b).ok())
            .and_then(|_| std::fs::rename(&tmp, &self.path).ok());
        if ok.is_none() {
            log::line("schedule: could not save schedule.json");
        }
    }

    /// Runs `f` on the list, saves, and wakes the thread to re-plan.
    pub fn edit<R>(&self, f: impl FnOnce(&mut List) -> R) -> R {
        let mut l = self.list.lock().unwrap();
        let r = f(&mut l);
        self.save(&l);
        self.wake.notify_all();
        r
    }

    pub fn snapshot(&self) -> Vec<Item> {
        self.list.lock().unwrap().sorted()
    }
}

/// Starts the thread that fires things on time.
pub fn start(app: AppHandle) {
    let spawned = std::thread::Builder::new().name("fedo-scheduler".into()).spawn(move || {
        let started = now();
        loop {
            let (fired, missed_cut) = {
                let sched = app.state::<Scheduler>();
                let mut l = sched.list.lock().unwrap();
                loop {
                    let at = now();
                    let due = l.take_due(at);
                    if !due.is_empty() {
                        sched.save(&l);
                        break (due, started);
                    }
                    // Sleep until the next item, or until something changes.
                    let wait = match l.next_due() {
                        Some(t) => Duration::from_secs((t - at).max(1) as u64),
                        None => Duration::from_secs(3600),
                    };
                    l = sched.wake.wait_timeout(l, wait).unwrap().0;
                }
            };
            for item in fired {
                // Late by more than a minute and it was due before we started: missed.
                let missed = item.fire_at < missed_cut - 60;
                fire(&app, item, missed);
            }
            let _ = app.emit("schedule-changed", ());
        }
    });
    if spawned.is_err() {
        log::line("scheduler thread did not start");
    }
}

fn fire(app: &AppHandle, item: Item, missed: bool) {
    log::line(format!("schedule: fired {:?} {:?} missed={missed}", item.kind, item.title));
    let (head, body) = match item.kind {
        Kind::Timer => ("Timer done".to_string(), item.title.clone()),
        _ => ("Reminder".to_string(), item.title.clone()),
    };
    let body = if missed { format!("{body} (while Fedo was off)") } else { body };
    notify::send(&head, &body);
    crate::island::emit(app, "schedule-fired", Fired { item, missed });
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn schedule_list(s: tauri::State<Scheduler>) -> Vec<Item> {
    s.snapshot()
}

#[tauri::command]
pub fn schedule_pause(app: AppHandle, s: tauri::State<Scheduler>, id: String) -> bool {
    let r = s.edit(|l| l.pause(&id, now()));
    let _ = app.emit("schedule-changed", ());
    r
}

#[tauri::command]
pub fn schedule_resume(app: AppHandle, s: tauri::State<Scheduler>, id: String) -> bool {
    let r = s.edit(|l| l.resume(&id, now()));
    let _ = app.emit("schedule-changed", ());
    r
}

#[tauri::command]
pub fn schedule_cancel(app: AppHandle, s: tauri::State<Scheduler>, id: String) -> bool {
    let r = s.edit(|l| l.cancel(&id));
    let _ = app.emit("schedule-changed", ());
    r
}

/// For the assistant: adds a timer of `seconds`.
pub fn add_timer(app: &AppHandle, seconds: i64, title: &str) -> String {
    let s = app.state::<Scheduler>();
    let id = s.edit(|l| {
        l.add(Item {
            kind: Kind::Timer,
            title: title.to_string(),
            fire_at: now() + seconds,
            total: Some(seconds),
            ..Default::default()
        })
    });
    let _ = app.emit("schedule-changed", ());
    id
}

pub fn add_reminder(app: &AppHandle, at: i64, title: &str) -> String {
    let s = app.state::<Scheduler>();
    let id = s.edit(|l| l.add(Item { kind: Kind::Reminder, title: title.to_string(), fire_at: at, ..Default::default() }));
    let _ = app.emit("schedule-changed", ());
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(at: i64, secs: i64) -> Item {
        Item { kind: Kind::Timer, title: "tea".into(), fire_at: at + secs, total: Some(secs), ..Default::default() }
    }

    #[test]
    fn things_fire_when_due_and_only_then() {
        let mut l = List::default();
        l.add(timer(1000, 60));
        l.add(Item { kind: Kind::Reminder, title: "oven".into(), fire_at: 1600, ..Default::default() });
        assert_eq!(l.next_due(), Some(1060));
        assert!(l.take_due(1059).is_empty());
        let due = l.take_due(1060);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].title, "tea");
        assert_eq!(l.next_due(), Some(1600));
        assert_eq!(l.take_due(9999).len(), 1);
        assert!(l.items.is_empty());
    }

    #[test]
    fn a_paused_timer_keeps_its_remaining_time() {
        let mut l = List::default();
        let id = l.add(timer(1000, 600));
        assert!(l.pause(&id, 1200));
        assert_eq!(l.items[0].paused_left, Some(400));
        assert_eq!(l.next_due(), None);
        assert!(l.take_due(99_999).is_empty());
        assert!(l.resume(&id, 5000));
        assert_eq!(l.items[0].fire_at, 5400);
        assert!(!l.resume(&id, 5001));
    }

    #[test]
    fn cancelling_a_deadline_takes_its_reminders_with_it() {
        let mut l = List::default();
        let d = l.add(Item { kind: Kind::Deadline, title: "Zomato".into(), fire_at: 5000, ..Default::default() });
        l.add(Item { kind: Kind::Reminder, title: "3 days".into(), fire_at: 100, parent: Some(d.clone()), ..Default::default() });
        l.add(Item { kind: Kind::Reminder, title: "other".into(), fire_at: 100, ..Default::default() });
        assert!(l.cancel(&d));
        assert_eq!(l.items.len(), 1);
        assert_eq!(l.items[0].title, "other");
    }

    #[test]
    fn deadlines_are_kept_a_day_past_and_ids_are_unique() {
        let mut l = List::default();
        let a = l.add(Item { kind: Kind::Deadline, fire_at: 1000, ..Default::default() });
        let b = l.add(Item { kind: Kind::Deadline, fire_at: 1000, ..Default::default() });
        assert_ne!(a, b);
        l.prune(1000 + 86_000);
        assert_eq!(l.items.len(), 2);
        l.prune(1000 + 86_401);
        assert!(l.items.is_empty());
    }

    #[test]
    fn the_list_survives_a_restart() {
        let path = std::env::temp_dir().join(format!("fedo-sched-{}.json", std::process::id()));
        let s = Scheduler { list: Mutex::new(List::default()), wake: Condvar::new(), path: path.clone() };
        s.edit(|l| l.add(timer(now(), 600)));
        s.edit(|l| l.add(Item { kind: Kind::Reminder, title: "call mum".into(), fire_at: now() + 99, ..Default::default() }));
        let again = load(&path);
        assert_eq!(again.items.len(), 2);
        assert_eq!(again.items[1].title, "call mum");
        assert_eq!(again.items[0].total, Some(600));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn something_that_fell_due_while_off_fires_on_the_first_look() {
        let mut l = List::default();
        l.add(Item { kind: Kind::Reminder, title: "late".into(), fire_at: 10, ..Default::default() });
        assert_eq!(l.take_due(1_000_000).len(), 1);
    }
}
