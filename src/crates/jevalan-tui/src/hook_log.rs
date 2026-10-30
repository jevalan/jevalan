//! Incremental reading of `shadow.jsonl` and aggregation of its decisions.
//! Note that everything here is pure except the reading of the file, and that it never keeps `tool_input`.

use jevalan_hub::lab::metrics::percentile;
use jevalan_hub::shadow::{Entry, HIGH_THRESHOLD, RISK_THRESHOLD, parse_line, suspicious};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Decisions retained for the list; the oldest are dropped.
pub const MAX_ENTRIES: usize = 500;

/// What each probe of the file returns.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Poll {
    /// The file was truncated, rotated or vanished: what has accumulated so far is no longer valid.
    pub reset: bool,
    /// Complete lines appended since the previous probe.
    pub lines: Vec<String>,
    /// The file exists.
    pub exists: bool,
    /// Size of the file, and how much of it has been read so far.
    pub total: u64,
    pub read: u64,
}

/// Follower of a file that only grows (a non-blocking `tail -f`).
#[derive(Debug)]
pub struct Tail {
    path: PathBuf,
    offset: u64,
    /// Remainder without a trailing newline: a half-written line.
    partial: Vec<u8>,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0, partial: Vec::new() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads what was added since the last time; if the file shrinks, it starts over from the beginning.
    pub fn poll(&mut self) -> Poll {
        self.poll_up_to(u64::MAX)
    }

    /// Like `poll`, but reads at most `limit` bytes, so that a large log can be taken in slices.
    pub fn poll_up_to(&mut self, limit: u64) -> Poll {
        let Ok(mut file) = File::open(&self.path) else {
            let reset = self.offset > 0;
            self.offset = 0;
            self.partial.clear();
            return Poll { reset, ..Poll::default() };
        };
        let len = file.metadata().map_or(0, |m| m.len());
        let mut poll = Poll { exists: true, total: len, ..Poll::default() };
        if len < self.offset {
            poll.reset = true;
            self.offset = 0;
            self.partial.clear();
        }
        poll.read = self.offset;
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return poll;
        }
        let mut chunk = Vec::new();
        if file.take((len - self.offset).min(limit)).read_to_end(&mut chunk).is_err() {
            return poll;
        }
        self.offset += chunk.len() as u64;
        poll.read = self.offset;
        self.partial.extend_from_slice(&chunk);
        while let Some(nl) = self.partial.iter().position(|b| *b == b'\n') {
            let rest = self.partial.split_off(nl + 1);
            let line = String::from_utf8_lossy(&self.partial[..nl]).trim().to_string();
            self.partial = rest;
            if !line.is_empty() {
                poll.lines.push(line);
            }
        }
        poll
    }
}

/// Class of a decision, for colour and counting purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Allow,
    Ask,
    Deny,
    Abstain,
    Error,
    /// `passthrough` or another verdict with no colour of its own.
    Other,
}

impl Kind {
    pub fn of(e: &Entry) -> Self {
        match (e.status.as_str(), e.verdict.as_deref()) {
            ("error", _) => Kind::Error,
            ("abstain", _) => Kind::Abstain,
            (_, Some("allow")) => Kind::Allow,
            (_, Some("ask")) => Kind::Ask,
            (_, Some("deny")) => Kind::Deny,
            _ => Kind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Allow => "allow",
            Kind::Ask => "ask",
            Kind::Deny => "deny",
            Kind::Abstain => "abstain",
            Kind::Error => "error",
            Kind::Other => "pass",
        }
    }
}

/// Counters and latencies of the whole log read.
#[derive(Debug, Default)]
pub struct Summary {
    pub total: usize,
    pub allow: usize,
    pub ask: usize,
    pub deny: usize,
    pub abstain: usize,
    pub error: usize,
    pub suspicious: usize,
    pub ignored: usize,
    /// Latencies of the calls with a response, sorted.
    latencies: Vec<u64>,
}

impl Summary {
    pub fn add(&mut self, e: &Entry) {
        self.total += 1;
        match Kind::of(e) {
            Kind::Allow => self.allow += 1,
            Kind::Ask => self.ask += 1,
            Kind::Deny => self.deny += 1,
            Kind::Abstain => self.abstain += 1,
            Kind::Error => self.error += 1,
            Kind::Other => {}
        }
        if suspicious(e) {
            self.suspicious += 1;
        }
        if e.status != "error"
            && let Some(ms) = e.latency_ms
        {
            let at = self.latencies.partition_point(|l| *l <= ms);
            self.latencies.insert(at, ms);
        }
    }

    pub fn p50(&self) -> Option<u64> {
        percentile(&self.latencies, 50.0)
    }

    pub fn p95(&self) -> Option<u64> {
        percentile(&self.latencies, 95.0)
    }
}

/// Band of a risk probability relative to the 0.55 and 0.85 thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Low,
    Mid,
    High,
}

pub fn level(p: f64) -> Level {
    if p >= HIGH_THRESHOLD {
        Level::High
    } else if p >= RISK_THRESHOLD {
        Level::Mid
    } else {
        Level::Low
    }
}

/// Geometry of a bar `width` cells wide: filled cells and positions of the 0.55 and 0.85 marks.
pub fn bar_geometry(p: f64, width: usize) -> (usize, [usize; 2]) {
    let tick = |x: f64| ((x * width as f64).round() as usize).min(width.saturating_sub(1));
    let filled = (p.clamp(0.0, 1.0) * width as f64).round() as usize;
    (filled.min(width), [tick(RISK_THRESHOLD), tick(HIGH_THRESHOLD)])
}

/// Live state of the log: follower, most recent decisions and summary.
#[derive(Debug)]
pub struct HookLog {
    tail: Tail,
    /// Oldest first.
    pub entries: VecDeque<Entry>,
    pub summary: Summary,
    pub exists: bool,
    /// Bytes of the file read so far, and its size at the last probe.
    read: u64,
    total: u64,
}

impl HookLog {
    pub fn new(path: PathBuf) -> Self {
        Self {
            tail: Tail::new(path),
            entries: VecDeque::new(),
            summary: Summary::default(),
            exists: false,
            read: 0,
            total: 0,
        }
    }

    /// Bytes read and bytes in the file, as of the last probe.
    pub fn progress(&self) -> (u64, u64) {
        (self.read, self.total)
    }

    /// Part of the file is still waiting to be read.
    pub fn loading(&self) -> bool {
        self.read < self.total
    }

    pub fn path(&self) -> &Path {
        self.tail.path()
    }

    /// Probes the file and returns how many new decisions arrived; `None` if nothing changed.
    pub fn refresh(&mut self) -> Option<usize> {
        self.refresh_up_to(u64::MAX)
    }

    /// `refresh` over at most `limit` bytes; call it again while `loading()` to take the rest.
    pub fn refresh_up_to(&mut self, limit: u64) -> Option<usize> {
        let poll = self.tail.poll_up_to(limit);
        let moved = poll.read != self.read || poll.total != self.total;
        let changed = poll.reset || poll.exists != self.exists || !poll.lines.is_empty() || moved;
        (self.read, self.total) = (poll.read, poll.total);
        self.exists = poll.exists;
        if poll.reset {
            self.entries.clear();
            self.summary = Summary::default();
        }
        let mut fresh = 0;
        for line in &poll.lines {
            match parse_line(line) {
                Some(mut e) => {
                    // The tool's input is not kept: screenshots end up on a public blog.
                    e.tool_input = None;
                    self.summary.add(&e);
                    if self.entries.len() == MAX_ENTRIES {
                        self.entries.pop_front();
                    }
                    self.entries.push_back(e);
                    fresh += 1;
                }
                None => self.summary.ignored += 1,
            }
        }
        changed.then_some(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("jevalan-hook-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn append(path: &Path, text: &str) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn line(verdict: &str, status: &str, ms: u64, risk: f64) -> String {
        format!(
            r#"{{"ts":"2026-01-01T10:00:00Z","tool_name":"Bash","tool_input":{{"command":"TOPSECRET"}},"status":"{status}","verdict":{verdict},"source":"model","probabilities":{{"destructive":{risk},"authorized":0.9}},"latency_ms":{ms}}}"#
        ) + "\n"
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let mut t = Tail::new(temp("missing"));
        assert_eq!(t.poll(), Poll::default());
    }

    #[test]
    fn reads_only_what_was_appended_and_waits_for_whole_lines() {
        let p = temp("incremental");
        append(&p, "uno\ndos\ntr");
        let mut t = Tail::new(p.clone());
        assert_eq!(t.poll().lines, ["uno", "dos"]);
        assert!(t.poll().lines.is_empty());
        append(&p, "es\ncuatro\n");
        let poll = t.poll();
        assert_eq!(poll.lines, ["tres", "cuatro"]);
        assert!(!poll.reset && poll.exists);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn truncation_rereads_from_the_start() {
        let p = temp("truncate");
        append(&p, "aaaaaaaa\nbbbbbbbb\n");
        let mut t = Tail::new(p.clone());
        assert_eq!(t.poll().lines.len(), 2);
        std::fs::write(&p, "c\n").unwrap();
        let poll = t.poll();
        assert!(poll.reset);
        assert_eq!(poll.lines, ["c"]);
        std::fs::remove_file(&p).unwrap();
        assert!(t.poll().reset);
    }

    #[test]
    fn summary_counts_percentiles_and_suspicious_asks() {
        let p = temp("summary");
        for (v, s, ms, risk) in [
            ("\"allow\"", "ok", 100, 0.1),
            ("\"allow\"", "ok", 200, 0.1),
            ("\"ask\"", "ok", 300, 0.3),
            ("\"ask\"", "ok", 400, 0.8),
            ("\"deny\"", "ok", 500, 0.9),
            ("null", "abstain", 600, 0.0),
            ("null", "error", 9999, 0.0),
        ] {
            append(&p, &line(v, s, ms, risk));
        }
        append(&p, "basura\n");
        let mut log = HookLog::new(p.clone());
        assert_eq!(log.refresh(), Some(7));
        let s = &log.summary;
        assert_eq!((s.total, s.allow, s.ask, s.deny, s.abstain, s.error), (7, 2, 2, 1, 1, 1));
        assert_eq!((s.suspicious, s.ignored), (1, 1));
        // The error does not enter the latencies: six samples remain, from 100 to 600.
        assert_eq!((s.p50(), s.p95()), (Some(300), Some(600)));
        assert!(log.entries.iter().all(|e| e.tool_input.is_none()));
        assert_eq!(log.refresh(), None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn retains_only_the_latest_entries() {
        let p = temp("cap");
        for _ in 0..MAX_ENTRIES + 5 {
            append(&p, &line("\"allow\"", "ok", 1, 0.0));
        }
        let mut log = HookLog::new(p.clone());
        log.refresh();
        assert_eq!((log.entries.len(), log.summary.total), (MAX_ENTRIES, MAX_ENTRIES + 5));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn bar_geometry_places_fill_and_ticks() {
        assert_eq!(bar_geometry(0.0, 20), (0, [11, 17]));
        assert_eq!(bar_geometry(0.5, 20), (10, [11, 17]));
        assert_eq!(bar_geometry(1.0, 20), (20, [11, 17]));
        assert_eq!(bar_geometry(7.0, 20).0, 20);
        assert_eq!(bar_geometry(0.9, 1), (1, [0, 0]));
        assert_eq!(level(0.54), Level::Low);
        assert_eq!(level(0.55), Level::Mid);
        assert_eq!(level(0.85), Level::High);
    }
}
