use serde::{Deserialize, Serialize};
use std::io::BufRead;
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
    Output(String, String), // (name, line)
    Finished(String),
    Failed(String, Vec<String>), // (name, last N error lines)
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
                            let _ = tx.send(BuildMsg::Output(name.clone(), line));
                        }
                    }
                }

                match child.wait() {
                    Ok(status) if status.success() => {
                        let _ = tx.send(BuildMsg::Finished(name.clone()));
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
                        let _ = tx.send(BuildMsg::Failed(name.clone(), error_lines));
                    }
                    Err(e) => {
                        let _ = tx.send(BuildMsg::Failed(name.clone(), vec![e.to_string()]));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(BuildMsg::Failed(name.clone(), vec![e.to_string()]));
            }
        }
    }

    let _ = tx.send(BuildMsg::QueueComplete);
}
