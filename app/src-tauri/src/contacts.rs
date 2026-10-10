// The phone's contacts (FEDO.md Phase 6): KDE Connect's contacts plugin keeps
// the phone's address book as vCards under ~/.local/share/kpeoplevcard/. This
// reads them into a name → numbers/emails index and matches fuzzy names
// ("pulkit" → Pulkit Sharma). Nothing is sent anywhere; the index lives in
// memory, rebuilt when the directory changes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub name: String,
    pub numbers: Vec<String>,
    pub emails: Vec<String>,
}

// ── vCard ─────────────────────────────────────────────────────────────────────

/// Quoted-printable text (`=F0=9F=8C=9D`), as KDE Connect's v2.1 cards write
/// non-ASCII names.
fn decode_qp(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' && i + 2 < b.len() + 0 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Joins folded lines: a continuation starts with space/tab, and a
/// quoted-printable line ending in `=` continues on the next one.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut qp_open = false;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if qp_open {
            if let Some(last) = lines.last_mut() {
                last.push_str(line);
            }
            qp_open = line.ends_with('=');
            if qp_open {
                if let Some(last) = lines.last_mut() {
                    last.pop();
                }
            }
            continue;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(line.trim_start());
            continue;
        }
        let upper = line.to_ascii_uppercase();
        qp_open = upper.contains("QUOTED-PRINTABLE") && line.ends_with('=');
        let mut l = line.to_string();
        if qp_open {
            l.pop();
        }
        lines.push(l);
    }
    lines
}

/// Reads one or more cards out of `text`.
pub fn parse_vcards(text: &str) -> Vec<Contact> {
    let mut out = Vec::new();
    let mut cur: Option<Contact> = None;
    let mut n_field = String::new();
    for line in unfold(text) {
        let Some((head, value)) = line.split_once(':') else { continue };
        let mut parts = head.split(';');
        let prop = parts.next().unwrap_or("").to_ascii_uppercase();
        let params: Vec<String> = parts.map(|p| p.to_ascii_uppercase()).collect();
        let qp = params.iter().any(|p| p.contains("QUOTED-PRINTABLE"));
        let value = if qp { decode_qp(value) } else { value.to_string() };
        match prop.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VCARD") => {
                cur = Some(Contact { name: String::new(), numbers: vec![], emails: vec![] });
                n_field.clear();
            }
            "END" if value.eq_ignore_ascii_case("VCARD") => {
                if let Some(mut c) = cur.take() {
                    if c.name.trim().is_empty() {
                        c.name = n_field.split(';').filter(|s| !s.is_empty()).rev().collect::<Vec<_>>().join(" ");
                    }
                    if !c.name.trim().is_empty() && (!c.numbers.is_empty() || !c.emails.is_empty()) {
                        out.push(c);
                    }
                }
            }
            "FN" => {
                if let Some(c) = cur.as_mut() {
                    c.name = value.trim().to_string();
                }
            }
            "N" => n_field = value,
            "TEL" => {
                if let Some(c) = cur.as_mut() {
                    let n = value.trim().to_string();
                    if !n.is_empty() && !c.numbers.contains(&n) {
                        c.numbers.push(n);
                    }
                }
            }
            "EMAIL" => {
                if let Some(c) = cur.as_mut() {
                    let e = value.trim().to_string();
                    if e.contains('@') && !c.emails.contains(&e) {
                        c.emails.push(e);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ── Matching ──────────────────────────────────────────────────────────────────

/// Lowercase, letters and digits only, one space between words.
pub fn norm(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    out
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// How well `query` names `name`: higher is better, 0 = not at all.
/// 100 the same name · 80 every query word is a whole word of the name ·
/// 60 each is a word's start · 40 within one typo per word · 0 otherwise.
/// Shorter names win ties (the contact called just "Aparna" beats "Aparna
/// College Mate").
pub fn score(query: &str, name: &str) -> u32 {
    let q = norm(query);
    let n = norm(name);
    if q.is_empty() || n.is_empty() {
        return 0;
    }
    if q == n {
        return 100;
    }
    let qw: Vec<&str> = q.split(' ').collect();
    let nw: Vec<&str> = n.split(' ').collect();
    let extra = nw.len().saturating_sub(qw.len()).min(9) as u32;
    let all = |f: &dyn Fn(&str, &str) -> bool| qw.iter().all(|w| nw.iter().any(|x| f(w, x)));
    if all(&|w, x| w == x) {
        return 80 - extra;
    }
    if all(&|w, x| w.len() >= 2 && x.starts_with(w)) {
        return 60 - extra;
    }
    if all(&|w, x| w.len() >= 4 && edit_distance(w, x) <= 1) {
        return 40 - extra;
    }
    0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub contact: Contact,
    pub score: u32,
}

/// Contacts that fit `query`, best first (at most `limit`).
pub fn find(contacts: &[Contact], query: &str, limit: usize) -> Vec<Found> {
    let mut v: Vec<Found> = contacts
        .iter()
        .map(|c| Found { contact: c.clone(), score: score(query, &c.name) })
        .filter(|f| f.score > 0)
        .collect();
    v.sort_by(|a, b| b.score.cmp(&a.score).then(a.contact.name.len().cmp(&b.contact.name.len())));
    // The same person saved twice is one candidate.
    let mut seen: Vec<String> = Vec::new();
    v.retain(|f| {
        let key = format!("{}|{}", norm(&f.contact.name), f.contact.numbers.join(","));
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    v.truncate(limit);
    v
}

/// One clear winner, or the few the user should choose between.
pub enum Pick {
    None,
    One(Contact),
    Many(Vec<Contact>),
}

pub fn pick(contacts: &[Contact], query: &str) -> Pick {
    let found = find(contacts, query, 6);
    match found.as_slice() {
        [] => Pick::None,
        [only] => Pick::One(only.contact.clone()),
        [first, second, ..] => {
            // An exact name that nothing else equals is the one meant.
            if first.score == 100 && second.score < 100 {
                Pick::One(first.contact.clone())
            } else {
                let top = first.score;
                Pick::Many(found.iter().filter(|f| f.score + 20 >= top).map(|f| f.contact.clone()).collect())
            }
        }
    }
}

/// A number as digits for a `wa.me` / `whatsapp://` link, or None when it has
/// no country code and none is known.
pub fn international_digits(number: &str, default_cc: &str) -> Option<String> {
    let trimmed = number.trim();
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return None;
    }
    if trimmed.starts_with('+') {
        return Some(digits);
    }
    if let Some(d) = digits.strip_prefix("00") {
        return Some(d.to_string());
    }
    if default_cc.is_empty() {
        return None;
    }
    let local = digits.trim_start_matches('0');
    if digits.starts_with(default_cc) && digits.len() >= default_cc.len() + 8 {
        return Some(digits);
    }
    Some(format!("{default_cc}{local}"))
}

/// A number safe to hand to a command: digits, +, and nothing else.
pub fn dialable(number: &str) -> String {
    let mut s = String::new();
    for (i, c) in number.trim().chars().enumerate() {
        if c.is_ascii_digit() || (c == '+' && i == 0) {
            s.push(c);
        }
    }
    s
}

// ── The index on disk ─────────────────────────────────────────────────────────

pub fn vcard_dir() -> PathBuf {
    crate::platform::home_dir().join(".local/share/kpeoplevcard")
}

fn load_from(dir: &Path) -> Vec<Contact> {
    let mut out = Vec::new();
    let Ok(devices) = std::fs::read_dir(dir) else { return out };
    for dev in devices.flatten() {
        if !dev.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dev.path()) else { continue };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().and_then(|e| e.to_str()) != Some("vcf") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.extend(parse_vcards(&text));
            }
        }
    }
    out
}

static CACHE: Mutex<Option<(SystemTime, usize, Vec<Contact>)>> = Mutex::new(None);

/// All contacts, cached until the directory changes.
pub fn all() -> Vec<Contact> {
    let dir = vcard_dir();
    let stamp = std::fs::metadata(&dir).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
    // A phone's sync adds files inside a device folder, not to `dir` itself.
    let count = std::fs::read_dir(&dir)
        .map(|d| {
            d.flatten()
                .map(|e| std::fs::metadata(e.path()).and_then(|m| m.modified()).map(|t| t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as usize).unwrap_or(0)).unwrap_or(0))
                .sum::<usize>()
        })
        .unwrap_or(0);
    let mut cache = CACHE.lock().unwrap();
    if let Some((s, c, list)) = cache.as_ref() {
        if *s == stamp && *c == count {
            return list.clone();
        }
    }
    let list = load_from(&dir);
    *cache = Some((stamp, count, list.clone()));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = "BEGIN:VCARD\nVERSION:2.1\nN:Sharma;Pulkit;;;\nFN:Pulkit Sharma\nTEL;CELL:+919811518797\nTEL;PREF:+919811518797\nEMAIL:p@example.com\nEND:VCARD\n\
BEGIN:VCARD\nVERSION:2.1\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=F0=9F=8C=9D;=41=64=69=74=79=61;;;\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=41=64=69=74=79=61=20=F0=9F=8C=9D\nTEL;CELL:812-821-7114\nEND:VCARD\n\
BEGIN:VCARD\nVERSION:2.1\nN:;Mom;;;\nFN:Mom\nTEL:098765 43210\nEND:VCARD\n\
BEGIN:VCARD\nVERSION:2.1\nFN:No Number\nEND:VCARD\n";

    fn people() -> Vec<Contact> {
        let mut v = parse_vcards(CARDS);
        for (n, t) in [("Aparna", "+911111111111"), ("Aparna Singh", "+912222222222"), ("Aparna College Mate", "+913333333333"), ("Aparna<33", "+914444444444"), ("Pulkit Dad", "+915555555555")] {
            v.push(Contact { name: n.into(), numbers: vec![t.into()], emails: vec![] });
        }
        v
    }

    #[test]
    fn cards_are_read_with_quoted_printable_and_duplicates_folded() {
        let c = parse_vcards(CARDS);
        assert_eq!(c.len(), 3, "a card with no number or email is not a contact");
        assert_eq!(c[0].name, "Pulkit Sharma");
        assert_eq!(c[0].numbers, vec!["+919811518797"]);
        assert_eq!(c[0].emails, vec!["p@example.com"]);
        assert_eq!(c[1].name, "Aditya 🌝");
        assert_eq!(c[2].numbers, vec!["098765 43210"]);
    }

    #[test]
    fn a_card_without_fn_falls_back_to_the_n_field() {
        let c = parse_vcards("BEGIN:VCARD\nN:Rao;Meera;;;\nTEL:123456789\nEND:VCARD\n");
        assert_eq!(c[0].name, "Meera Rao");
    }

    #[test]
    fn names_match_fuzzily() {
        let p = people();
        assert_eq!(find(&p, "pulkit sharma", 3)[0].contact.name, "Pulkit Sharma");
        // A typo still finds a Pulkit.
        assert!(find(&p, "pulkt", 3)[0].contact.name.starts_with("Pulkit"));
        assert_eq!(score("mom", "Mom"), 100);
        assert_eq!(score("zzz", "Mom"), 0);
        assert!(score("adi", "Aditya 🌝") > 0);
    }

    #[test]
    fn an_exact_name_is_the_one_meant_and_a_shared_first_name_asks() {
        let p = people();
        match pick(&p, "aparna") {
            Pick::One(c) => assert_eq!(c.name, "Aparna"),
            _ => panic!("the contact called exactly Aparna should win"),
        }
        match pick(&p, "pulkit") {
            Pick::Many(v) => {
                let names: Vec<_> = v.iter().map(|c| c.name.as_str()).collect();
                assert!(names.contains(&"Pulkit Sharma") && names.contains(&"Pulkit Dad"), "{names:?}");
            }
            _ => panic!("two Pulkits: ask"),
        }
        assert!(matches!(pick(&p, "nobody here"), Pick::None));
    }

    #[test]
    fn numbers_become_what_whatsapp_and_the_dialer_need() {
        assert_eq!(international_digits("+91 98115-18797", "").as_deref(), Some("919811518797"));
        assert_eq!(international_digits("98115 18797", "91").as_deref(), Some("919811518797"));
        assert_eq!(international_digits("098115 18797", "91").as_deref(), Some("919811518797"));
        assert_eq!(international_digits("919811518797", "91").as_deref(), Some("919811518797"));
        assert_eq!(international_digits("98115 18797", ""), None);
        assert_eq!(international_digits("0044 20 7946 0958", "").as_deref(), Some("442079460958"));
        assert_eq!(international_digits("123", "91"), None);
        assert_eq!(dialable("+91 (981) 15-18797; rm -rf"), "+919811518797");
    }

    #[test]
    fn the_real_address_book_loads_when_present() {
        // Whatever this machine has: it must parse without panicking and every
        // contact must have a name and something to reach them by.
        for c in load_from(&vcard_dir()) {
            assert!(!c.name.trim().is_empty());
            assert!(!c.numbers.is_empty() || !c.emails.is_empty());
        }
    }
}
