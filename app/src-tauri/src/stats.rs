// `fedo stats`: what Fedo costs right now (FEDO.md §3).
//
// Reads /proc for every process of the running daemon — the daemon itself and
// the WebKit processes it spawned — and prints resident memory and CPU. It
// never talks to the daemon, so it works (and says so) when none is running.

use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};

struct Proc {
    pid: u32,
    ppid: u32,
    name: String,
    /// utime + stime, clock ticks.
    ticks: u64,
}

fn read_proc(pid: u32) -> Option<Proc> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (comm with spaces) S ppid ..." — comm may hold spaces and parens.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let rest: Vec<&str> = stat[close + 2..].split_whitespace().collect();
    // rest[0] = state, [1] = ppid, [11] = utime, [12] = stime
    let ppid = rest.get(1)?.parse().ok()?;
    let ticks = rest.get(11)?.parse::<u64>().ok()? + rest.get(12)?.parse::<u64>().ok()?;
    Some(Proc { pid, ppid, name, ticks })
}

fn all_procs() -> HashMap<u32, Proc> {
    let mut out = HashMap::new();
    if let Ok(dir) = fs::read_dir("/proc") {
        for entry in dir.flatten() {
            if let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) {
                if let Some(p) = read_proc(pid) {
                    out.insert(pid, p);
                }
            }
        }
    }
    out
}

/// The daemon (same executable as this one, not this process) and everything
/// below it.
fn fedo_tree(procs: &HashMap<u32, Proc>) -> Vec<u32> {
    let me = std::process::id();
    let exe = fs::read_link("/proc/self/exe").ok();
    let mut roots: Vec<u32> = procs
        .keys()
        .copied()
        .filter(|&pid| pid != me && fs::read_link(format!("/proc/{pid}/exe")).ok() == exe)
        .collect();
    roots.sort_unstable();
    let mut tree = roots.clone();
    let mut frontier = roots;
    while !frontier.is_empty() {
        let next: Vec<u32> = procs
            .values()
            .filter(|p| frontier.contains(&p.ppid) && !tree.contains(&p.pid))
            .map(|p| p.pid)
            .collect();
        tree.extend(&next);
        frontier = next;
    }
    tree
}

/// (rss kB, pss kB) — PSS splits shared pages between their users, so the sum
/// is not inflated by the libraries every WebKit process maps.
fn memory(pid: u32) -> (u64, u64) {
    let field = |text: &str, key: &str| -> u64 {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    let rollup = fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
    (field(&status, "VmRSS:"), field(&rollup, "Pss:"))
}

pub fn run() -> i32 {
    let first = all_procs();
    let tree = fedo_tree(&first);
    if tree.is_empty() {
        println!("fedo is not running");
        return 1;
    }
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(1000));
    let second = all_procs();
    let secs = started.elapsed().as_secs_f64();
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;

    println!("{:>7}  {:<18} {:>9} {:>9} {:>6}", "PID", "PROCESS", "RSS MB", "PSS MB", "CPU %");
    let (mut rss, mut pss, mut cpu) = (0u64, 0u64, 0.0f64);
    for pid in tree {
        let (Some(a), Some(b)) = (first.get(&pid), second.get(&pid)) else { continue };
        let (r, p) = memory(pid);
        let c = (b.ticks.saturating_sub(a.ticks)) as f64 / hz / secs * 100.0;
        println!("{:>7}  {:<18} {:>9.1} {:>9.1} {:>6.1}", pid, b.name, r as f64 / 1024.0, p as f64 / 1024.0, c);
        rss += r;
        pss += p;
        cpu += c;
    }
    println!("{:>7}  {:<18} {:>9.1} {:>9.1} {:>6.1}", "", "TOTAL", rss as f64 / 1024.0, pss as f64 / 1024.0, cpu);
    0
}
