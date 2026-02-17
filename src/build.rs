use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq)]
pub enum BuildJobStatus {
    Pending,
    Building,
    Success,
    Failed,
}

#[derive(Debug, Clone)]
pub struct BuildJob {
    pub name: String,
    pub status: BuildJobStatus,
}

pub enum BuildMsg {
    Started(String),
    Output(String, String),                    // (name, line)
    Finished(String, PathBuf),                 // (name, log_path)
    Failed(String, Vec<String>, PathBuf),      // (name, last N error lines, log_path)
    QueueComplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildHistoryEntry {
    pub name: String,
    pub success: bool,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildHistory {
    pub entries: Vec<BuildHistoryEntry>,
}

impl BuildHistory {
    pub fn load() -> Self {
        let path = Self::path();
        if let Ok(data) = std::fs::read_to_string(&path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, data);
        }
    }

    pub fn record(&mut self, name: &str, success: bool) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.entries.push(BuildHistoryEntry {
            name: name.to_string(),
            success,
            timestamp,
        });
        self.save();
    }

    fn path() -> PathBuf {
        let cache = std::env::var("XDG_CACHE_HOME")
            .unwrap_or_else(|_| format!("{}/.cache", env!("HOME")));
        PathBuf::from(cache).join("vpm/build_history.json")
    }
}

// --- Build Log Persistence ---

struct BuildLog {
    file: std::fs::File,
    path: PathBuf,
}

impl BuildLog {
    fn new(pkg_name: &str) -> Option<Self> {
        let dir = log_dir();
        let _ = std::fs::create_dir_all(&dir);

        let now = chrono_timestamp();
        let filename = format!("{}-{}.log", pkg_name, now);
        let path = dir.join(filename);

        match std::fs::File::create(&path) {
            Ok(file) => Some(BuildLog { file, path }),
            Err(_) => None,
        }
    }

    fn write_line(&mut self, line: &str) {
        let _ = writeln!(self.file, "{}", line);
    }

    fn finish(self) -> PathBuf {
        self.path
    }
}

fn log_dir() -> PathBuf {
    let cache = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", env!("HOME")));
    PathBuf::from(cache).join("vpm/logs")
}

fn chrono_timestamp() -> String {
    // YYYYMMDD-HHMMSS without pulling in chrono crate
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Simple UTC conversion
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y/M/D (simplified Gregorian)
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Prune old build logs, keeping at most `max_per_pkg` per package.
pub fn prune_build_logs(max_per_pkg: usize) {
    let dir = log_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut by_pkg: HashMap<String, Vec<PathBuf>> = HashMap::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "log").unwrap_or(false) {
            if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                // Format: pkgname-YYYYMMDD-HHMMSS
                // Find the timestamp part (last 15 chars: YYYYMMDD-HHMMSS)
                if filename.len() > 16 {
                    let pkg = &filename[..filename.len() - 16]; // strip "-YYYYMMDD-HHMMSS"
                    by_pkg.entry(pkg.to_string()).or_default().push(path);
                }
            }
        }
    }

    for (_pkg, mut logs) in by_pkg {
        if logs.len() <= max_per_pkg {
            continue;
        }
        logs.sort();
        // Oldest first (filenames sort chronologically), remove excess
        let to_remove = logs.len() - max_per_pkg;
        for path in &logs[..to_remove] {
            let _ = std::fs::remove_file(path);
        }
    }
}

// --- Build Queue ---

pub struct BuildQueue {
    pub jobs: Vec<BuildJob>,
    pub current_output: Vec<String>,
    pub receiver: Option<Receiver<BuildMsg>>,
    pub active: bool,
    pub cancel_flag: Arc<AtomicBool>,
}

impl BuildQueue {
    pub fn new() -> Self {
        BuildQueue {
            jobs: Vec::new(),
            current_output: Vec::new(),
            receiver: None,
            active: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start building the queued jobs in a background thread.
    pub fn start(&mut self, void_pkgs: PathBuf) {
        if self.jobs.is_empty() {
            return;
        }

        self.active = true;
        self.current_output.clear();
        self.cancel_flag.store(false, Ordering::SeqCst);

        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);

        let names: Vec<String> = self.jobs.iter().map(|j| j.name.clone()).collect();
        let cancel = self.cancel_flag.clone();

        std::thread::spawn(move || {
            run_build_queue(void_pkgs, names, tx, cancel);
        });
    }
}

fn run_build_queue(void_pkgs: PathBuf, names: Vec<String>, tx: Sender<BuildMsg>, cancel: Arc<AtomicBool>) {
    for name in &names {
        if cancel.load(Ordering::SeqCst) {
            break;
        }

        let _ = tx.send(BuildMsg::Started(name.clone()));
        let mut build_log = BuildLog::new(name);

        let result = Command::new("./xbps-src")
            .args(["pkg", name])
            .current_dir(&void_pkgs)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        match result {
            Ok(mut child) => {
                // Read stdout line by line
                if let Some(stdout) = child.stdout.take() {
                    let reader = std::io::BufReader::new(stdout);
                    for line in reader.lines() {
                        if cancel.load(Ordering::SeqCst) {
                            let _ = child.kill();
                            break;
                        }
                        if let Ok(line) = line {
                            if let Some(ref mut log) = build_log {
                                log.write_line(&line);
                            }
                            let _ = tx.send(BuildMsg::Output(name.clone(), line));
                        }
                    }
                }

                let log_path = build_log.map(|l| l.finish()).unwrap_or_default();

                match child.wait() {
                    Ok(status) if status.success() => {
                        let _ = tx.send(BuildMsg::Finished(name.clone(), log_path));
                    }
                    Ok(_) => {
                        // Capture stderr for error info
                        let error_lines = if let Some(stderr) = child.stderr.take() {
                            let reader = std::io::BufReader::new(stderr);
                            let all: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
                            let start = all.len().saturating_sub(50);
                            all[start..].to_vec()
                        } else {
                            vec!["Build failed with non-zero exit code".to_string()]
                        };
                        // Write errors to log too
                        if log_path != PathBuf::new() {
                            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path) {
                                for line in &error_lines {
                                    let _ = writeln!(f, "ERR: {}", line);
                                }
                            }
                        }
                        let _ = tx.send(BuildMsg::Failed(name.clone(), error_lines, log_path));
                    }
                    Err(e) => {
                        let _ = tx.send(BuildMsg::Failed(name.clone(), vec![e.to_string()], log_path));
                    }
                }
            }
            Err(e) => {
                let log_path = build_log.map(|l| l.finish()).unwrap_or_default();
                let _ = tx.send(BuildMsg::Failed(name.clone(), vec![e.to_string()], log_path));
            }
        }
    }

    let _ = tx.send(BuildMsg::QueueComplete);
}
