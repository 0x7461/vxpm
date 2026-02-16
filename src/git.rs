use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct GitStatus {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub last_fetch: Option<SystemTime>,
}

pub enum GitMsg {
    Output(String),
    Success(String),
    Failed(String),
    Done,
}

#[derive(Debug, Clone, Copy)]
pub enum GitOp {
    SyncMaster,
    RebaseCustom,
    PushCustom,
}

pub fn get_git_status(void_pkgs: &Path) -> Option<GitStatus> {
    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(void_pkgs)
        .output()
        .ok()?;
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();

    let counts = Command::new("git")
        .args(["rev-list", "--left-right", "--count", "master...custom"])
        .current_dir(void_pkgs)
        .output()
        .ok()?;
    let counts_str = String::from_utf8_lossy(&counts.stdout).trim().to_string();
    let parts: Vec<&str> = counts_str.split('\t').collect();
    let (behind, ahead) = if parts.len() == 2 {
        (
            parts[0].parse().unwrap_or(0),
            parts[1].parse().unwrap_or(0),
        )
    } else {
        (0, 0)
    };

    let fetch_head = void_pkgs.join(".git/FETCH_HEAD");
    let last_fetch = std::fs::metadata(&fetch_head)
        .ok()
        .and_then(|m| m.modified().ok());

    Some(GitStatus {
        branch,
        ahead,
        behind,
        last_fetch,
    })
}

pub fn run_git_op(void_pkgs: PathBuf, op: GitOp, tx: Sender<GitMsg>) {
    match op {
        GitOp::SyncMaster => {
            let _ = tx.send(GitMsg::Output("Fetching void/master...".into()));
            if !run_streaming(&void_pkgs, &["fetch", "void", "master"], &tx) {
                let _ = tx.send(GitMsg::Failed("fetch failed".into()));
                let _ = tx.send(GitMsg::Done);
                return;
            }
            let _ = tx.send(GitMsg::Output(
                "Updating master ref to void/master...".into(),
            ));
            let out = Command::new("git")
                .args([
                    "update-ref",
                    "refs/heads/master",
                    "refs/remotes/void/master",
                ])
                .current_dir(&void_pkgs)
                .output();
            match out {
                Ok(o) if o.status.success() => {
                    let _ = tx.send(GitMsg::Success("Master synced to upstream".into()));
                }
                _ => {
                    let _ = tx.send(GitMsg::Failed("update-ref failed".into()));
                }
            }
            let _ = tx.send(GitMsg::Done);
        }
        GitOp::RebaseCustom => {
            let _ = tx.send(GitMsg::Output("Rebasing custom onto master...".into()));
            if !run_streaming(&void_pkgs, &["rebase", "master"], &tx) {
                let _ = tx.send(GitMsg::Output("Rebase failed, aborting...".into()));
                let _ = Command::new("git")
                    .args(["rebase", "--abort"])
                    .current_dir(&void_pkgs)
                    .output();
                let _ = tx.send(GitMsg::Failed("Rebase failed (aborted)".into()));
                let _ = tx.send(GitMsg::Done);
                return;
            }
            let _ = tx.send(GitMsg::Success("Rebase complete".into()));
            let _ = tx.send(GitMsg::Done);
        }
        GitOp::PushCustom => {
            let _ = tx.send(GitMsg::Output("Pushing custom (--force-with-lease)...".into()));
            if !run_streaming(
                &void_pkgs,
                &["push", "origin", "custom", "--force-with-lease"],
                &tx,
            ) {
                let _ = tx.send(GitMsg::Failed("Push failed".into()));
                let _ = tx.send(GitMsg::Done);
                return;
            }
            let _ = tx.send(GitMsg::Success("Push complete".into()));
            let _ = tx.send(GitMsg::Done);
        }
    }
}

/// Run a git command, streaming stdout+stderr line by line. Returns true if exit code == 0.
fn run_streaming(void_pkgs: &Path, args: &[&str], tx: &Sender<GitMsg>) -> bool {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut child = match Command::new("git")
        .args(args)
        .current_dir(void_pkgs)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(GitMsg::Output(format!("Failed to spawn git: {}", e)));
            return false;
        }
    };

    // Read stdout in a thread
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let tx2 = tx.clone();

    let stdout_handle = std::thread::spawn(move || {
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx2.send(GitMsg::Output(line));
            }
        }
    });

    if let Some(err) = stderr {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send(GitMsg::Output(line));
        }
    }

    let _ = stdout_handle.join();
    matches!(child.wait(), Ok(status) if status.success())
}
