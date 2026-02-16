use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use ratatui::widgets::TableState;

use crate::build::{BuildHistory, BuildJob, BuildJobStatus, BuildMsg, BuildQueue};
use crate::dep_graph::DepGraph;
use crate::gcc::GccInfo;
use crate::git::{self, GitMsg, GitStatus};
use crate::package::{Package, PackageState, Status};
use crate::repo;
use crate::shlibs::{self, ShlibMap};
use crate::template;
use crate::version_check;

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    List,
    Tree,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PanelMode {
    None,
    Detail,
    BuildLog,
    GitMenu,
}

pub struct App {
    pub packages: Vec<PackageState>,
    pub selected: usize,
    pub view: View,
    pub dep_graph: DepGraph,
    pub void_pkgs: PathBuf,
    pub panel: PanelMode,
    pub checking_versions: bool,
    pub status_msg: Option<String>,
    pub should_quit: bool,
    pub build_queue: BuildQueue,
    pub build_history: BuildHistory,
    pub cancel_pending: Option<Instant>,
    pub version_check_rx: Option<Receiver<version_check::VersionMsg>>,
    pub git_status: Option<GitStatus>,
    pub git_op_rx: Option<Receiver<GitMsg>>,
    pub git_output: Vec<String>,
    pub git_op_active: bool,
    pub table_state: TableState,
    pub filter: String,
    pub filter_active: bool,
    pub shlib_map: ShlibMap,
    pub gcc_info: GccInfo,
}

impl App {
    pub fn new(void_pkgs: PathBuf) -> anyhow::Result<Self> {
        let names = repo::discover_custom_packages(&void_pkgs)?;
        let packages = repo::load_packages(&void_pkgs, &names);
        let dep_graph = DepGraph::build(&packages);
        let mut states = repo::build_package_states(&void_pkgs, packages);

        let git_status = git::get_git_status(&void_pkgs);
        let shlib_map = shlibs::parse_shlibs(&void_pkgs);
        let gcc_info = GccInfo::detect();

        // Populate shlibs and check mismatches
        for state in &mut states {
            if let Some(entries) = shlib_map.get(&state.package.name) {
                state.shlibs = entries.clone();
                state.soname_mismatches =
                    shlibs::check_soname_mismatches(entries, &state.package.name);
            }
        }

        Ok(App {
            packages: states,
            selected: 0,
            view: View::List,
            dep_graph,
            void_pkgs,
            panel: PanelMode::None,
            checking_versions: false,
            status_msg: Some("Press 'u' to check upstream versions".to_string()),
            should_quit: false,
            build_queue: BuildQueue::new(),
            build_history: BuildHistory::load(),
            cancel_pending: None,
            version_check_rx: None,
            git_status,
            git_op_rx: None,
            git_output: Vec::new(),
            git_op_active: false,
            table_state: TableState::default(),
            filter: String::new(),
            filter_active: false,
            shlib_map,
            gcc_info,
        })
    }

    /// Returns (original_index, &PackageState) for packages matching the current filter.
    pub fn visible_packages(&self) -> Vec<(usize, &PackageState)> {
        if self.filter.is_empty() {
            self.packages.iter().enumerate().collect()
        } else {
            let filter_lower = self.filter.to_lowercase();
            self.packages
                .iter()
                .enumerate()
                .filter(|(_, p)| p.package.name.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    pub fn selected_package(&self) -> Option<&PackageState> {
        let visible = self.visible_packages();
        visible.get(self.selected).map(|(_, p)| *p)
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let visible_len = self.visible_packages().len();
        if self.selected + 1 < visible_len {
            self.selected += 1;
        }
    }

    pub fn start_filter(&mut self) {
        self.filter_active = true;
        self.filter.clear();
        self.selected = 0;
    }

    pub fn filter_input(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub fn filter_backspace(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    pub fn stop_filter(&mut self, clear: bool) {
        self.filter_active = false;
        if clear {
            self.filter.clear();
            self.selected = 0;
        }
    }

    pub fn toggle_detail(&mut self) {
        self.panel = match self.panel {
            PanelMode::Detail => PanelMode::None,
            _ => PanelMode::Detail,
        };
    }

    pub fn toggle_tree(&mut self) {
        self.view = match self.view {
            View::List => View::Tree,
            View::Tree => View::List,
        };
    }

    pub fn refresh(&mut self) {
        if let Ok(names) = repo::discover_custom_packages(&self.void_pkgs) {
            let packages = repo::load_packages(&self.void_pkgs, &names);
            self.dep_graph = DepGraph::build(&packages);
            let mut states = repo::build_package_states(&self.void_pkgs, packages);

            // Preserve latest versions and build-failed overrides from previous state
            let old_latest: std::collections::HashMap<String, String> = self
                .packages
                .iter()
                .filter_map(|p| {
                    p.latest
                        .as_ref()
                        .map(|v| (p.package.name.clone(), v.clone()))
                })
                .collect();

            let failed: std::collections::HashSet<String> = self
                .packages
                .iter()
                .filter(|p| p.status == Status::BuildFailed)
                .map(|p| p.package.name.clone())
                .collect();

            for state in &mut states {
                if let Some(latest) = old_latest.get(&state.package.name) {
                    state.latest = Some(latest.clone());
                    state.status = PackageState::compute_status(
                        &state.package,
                        &state.installed,
                        &state.built,
                        &state.latest,
                    );
                }
                if failed.contains(&state.package.name) {
                    state.status = Status::BuildFailed;
                }
            }

            // Re-parse shlibs and check mismatches
            self.shlib_map = shlibs::parse_shlibs(&self.void_pkgs);
            for state in &mut states {
                if let Some(entries) = self.shlib_map.get(&state.package.name) {
                    state.shlibs = entries.clone();
                    state.soname_mismatches =
                        shlibs::check_soname_mismatches(entries, &state.package.name);
                }
            }

            self.packages = states;
            let visible_len = self.visible_packages().len();
            if self.selected >= visible_len {
                self.selected = visible_len.saturating_sub(1);
            }
            self.status_msg = Some("Refreshed".to_string());
        }
    }

    pub fn check_versions(&mut self) {
        self.start_version_check(false);
    }

    pub fn force_check_versions(&mut self) {
        self.start_version_check(true);
    }

    fn start_version_check(&mut self, force: bool) {
        if self.checking_versions {
            return;
        }
        self.checking_versions = true;
        let label = if force { "Force-checking" } else { "Checking" };
        self.status_msg = Some(format!("{} upstream versions...", label));

        let pkgs: Vec<Package> = self.packages.iter().map(|s| s.package.clone()).collect();
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.version_check_rx = Some(rx);

        std::thread::spawn(move || {
            version_check::check_all_versions_streaming(&void_pkgs, &pkgs, force, tx);
        });
    }

    /// Poll for version check results. Call each tick.
    pub fn poll_version_check(&mut self) {
        let msgs: Vec<version_check::VersionMsg> = if let Some(ref rx) = self.version_check_rx {
            rx.try_iter().collect()
        } else {
            return;
        };

        for msg in msgs {
            match msg {
                version_check::VersionMsg::Found(name, ver) => {
                    for state in &mut self.packages {
                        if state.package.name == name {
                            state.latest = Some(ver.clone());
                            state.status = PackageState::compute_status(
                                &state.package,
                                &state.installed,
                                &state.built,
                                &state.latest,
                            );
                        }
                    }
                    self.status_msg = Some(format!("Checked {}...", name));
                }
                version_check::VersionMsg::Done(count) => {
                    self.checking_versions = false;
                    self.version_check_rx = None;
                    self.status_msg = Some(format!("Checked {} packages", count));
                    return;
                }
            }
        }
    }

    /// Poll the build queue for messages from the background thread.
    pub fn poll_build(&mut self) {
        if !self.build_queue.active {
            return;
        }

        let msgs: Vec<BuildMsg> = if let Some(ref rx) = self.build_queue.receiver {
            rx.try_iter().collect()
        } else {
            return;
        };

        for msg in msgs {
            match msg {
                BuildMsg::Started(name) => {
                    for job in &mut self.build_queue.jobs {
                        if job.name == name {
                            job.status = BuildJobStatus::Building;
                        }
                    }
                    self.build_queue.current_output.clear();
                    self.status_msg = Some(format!("Building {}...", name));
                }
                BuildMsg::Output(_name, line) => {
                    self.build_queue.current_output.push(line);
                    // Keep only last 200 lines in memory
                    if self.build_queue.current_output.len() > 200 {
                        let drain = self.build_queue.current_output.len() - 200;
                        self.build_queue.current_output.drain(..drain);
                    }
                }
                BuildMsg::Finished(name) => {
                    for job in &mut self.build_queue.jobs {
                        if job.name == name {
                            job.status = BuildJobStatus::Success;
                        }
                    }
                    self.build_history.record(&name, true);
                }
                BuildMsg::Failed(name, error_lines) => {
                    for job in &mut self.build_queue.jobs {
                        if job.name == name {
                            job.status = BuildJobStatus::Failed;
                        }
                    }
                    // Set BuildFailed status on the package
                    for state in &mut self.packages {
                        if state.package.name == name {
                            state.status = Status::BuildFailed;
                        }
                    }
                    // Append error lines to output
                    for line in &error_lines {
                        self.build_queue.current_output.push(format!("ERR: {}", line));
                    }
                    self.build_history.record(&name, false);
                }
                BuildMsg::QueueComplete => {
                    self.build_queue.active = false;
                    self.build_queue.receiver = None;

                    // Refresh to pick up new .xbps files
                    self.refresh();

                    // Build the xi command for successful packages
                    let succeeded: Vec<String> = self
                        .build_queue
                        .jobs
                        .iter()
                        .filter(|j| j.status == BuildJobStatus::Success)
                        .map(|j| j.name.clone())
                        .collect();

                    if !succeeded.is_empty() {
                        self.status_msg =
                            Some(format!("Run: xi {}", succeeded.join(" ")));
                    } else {
                        self.status_msg = Some("Build queue finished (no successful builds)".to_string());
                    }
                }
            }
        }
    }

    /// Build the currently selected package.
    pub fn build_selected(&mut self) {
        if self.build_queue.active {
            self.status_msg = Some("Build already in progress".to_string());
            return;
        }

        let name = match self.selected_package() {
            Some(p) => p.package.name.clone(),
            None => return,
        };

        if self.gcc_info.is_blocked(&name) {
            let req = self.gcc_info.required_version(&name).unwrap_or_default();
            self.status_msg = Some(format!(
                "Cannot build {}: requires GCC {}+, system has {}",
                name, req, self.gcc_info.version_string()
            ));
            return;
        }

        self.build_queue.jobs = vec![BuildJob {
            name,
            status: BuildJobStatus::Pending,
        }];
        self.build_queue.start(self.void_pkgs.clone());
        self.panel = PanelMode::BuildLog;
    }

    /// Build the selected package plus all reverse dependents in topological order.
    pub fn build_with_dependents(&mut self) {
        if self.build_queue.active {
            self.status_msg = Some("Build already in progress".to_string());
            return;
        }

        let name = match self.selected_package() {
            Some(p) => p.package.name.clone(),
            None => return,
        };

        let dependents = self.dep_graph.rebuild_set(&[name.clone()]);
        let mut all = vec![name];
        all.extend(dependents);

        // Check GCC requirements for all packages in queue
        let blocked: Vec<String> = all
            .iter()
            .filter(|n| self.gcc_info.is_blocked(n))
            .cloned()
            .collect();
        if !blocked.is_empty() {
            self.status_msg = Some(format!(
                "Cannot build: {} require newer GCC (system has {})",
                blocked.join(", "),
                self.gcc_info.version_string()
            ));
            return;
        }

        self.build_queue.jobs = all
            .into_iter()
            .map(|n| BuildJob {
                name: n,
                status: BuildJobStatus::Pending,
            })
            .collect();
        self.build_queue.start(self.void_pkgs.clone());
        self.panel = PanelMode::BuildLog;
    }

    /// Bump template version and queue a build (for UPDATE AVAIL packages).
    pub fn bump_and_build(&mut self) {
        if self.build_queue.active {
            self.status_msg = Some("Build already in progress".to_string());
            return;
        }

        let (name, latest) = match self.selected_package() {
            Some(p) if p.status == Status::UpdateAvailable => {
                let latest = p.latest.clone().unwrap_or_default();
                (p.package.name.clone(), latest)
            }
            _ => {
                // Not UPDATE AVAIL — fall through to force_check_versions
                self.force_check_versions();
                return;
            }
        };

        if self.gcc_info.is_blocked(&name) {
            let req = self.gcc_info.required_version(&name).unwrap_or_default();
            self.status_msg = Some(format!(
                "Cannot build {}: requires GCC {}+, system has {}",
                name, req, self.gcc_info.version_string()
            ));
            return;
        }

        self.status_msg = Some(format!("Bumping {} to v{}...", name, latest));

        match template::bump_template(&self.void_pkgs, &name, &latest) {
            Ok(result) => {
                self.status_msg = Some(format!(
                    "Bumped {} {} -> {} (checksum: {}...)",
                    name,
                    result.old_version,
                    result.new_version,
                    &result.new_checksum[..12]
                ));

                // Refresh to pick up the new template version
                self.refresh();

                // Now queue the build
                self.build_queue.jobs = vec![BuildJob {
                    name,
                    status: BuildJobStatus::Pending,
                }];
                self.build_queue.start(self.void_pkgs.clone());
                self.panel = PanelMode::BuildLog;
            }
            Err(e) => {
                self.status_msg = Some(format!("Bump failed: {}", e));
            }
        }
    }

    /// Handle cancel build: double-Esc within 2 seconds.
    pub fn cancel_build(&mut self) {
        if !self.build_queue.active {
            return;
        }

        if let Some(first_press) = self.cancel_pending {
            if first_press.elapsed().as_secs() < 2 {
                // Second press within 2s — cancel
                self.build_queue.cancel_flag.store(true, Ordering::SeqCst);
                self.cancel_pending = None;
                self.status_msg = Some("Cancelling build...".to_string());
            } else {
                // Expired — treat as first press again
                self.cancel_pending = Some(Instant::now());
                self.status_msg = Some("Press Esc again to cancel build".to_string());
            }
        } else {
            self.cancel_pending = Some(Instant::now());
            self.status_msg = Some("Press Esc again to cancel build".to_string());
        }
    }

    pub fn refresh_git_status(&mut self) {
        self.git_status = git::get_git_status(&self.void_pkgs);
    }

    pub fn open_git_menu(&mut self) {
        self.panel = match self.panel {
            PanelMode::GitMenu => PanelMode::None,
            _ => {
                self.refresh_git_status();
                PanelMode::GitMenu
            }
        };
    }

    fn start_git_op(&mut self, op: git::GitOp) {
        if self.git_op_active {
            self.status_msg = Some("Git operation already in progress".to_string());
            return;
        }
        self.git_op_active = true;
        self.git_output.clear();
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.git_op_rx = Some(rx);
        std::thread::spawn(move || {
            git::run_git_op(void_pkgs, op, tx);
        });
    }

    pub fn git_sync_master(&mut self) {
        self.start_git_op(git::GitOp::SyncMaster);
    }

    pub fn git_rebase_custom(&mut self) {
        self.start_git_op(git::GitOp::RebaseCustom);
    }

    pub fn git_push_custom(&mut self) {
        self.start_git_op(git::GitOp::PushCustom);
    }

    pub fn poll_git(&mut self) {
        let msgs: Vec<GitMsg> = if let Some(ref rx) = self.git_op_rx {
            rx.try_iter().collect()
        } else {
            return;
        };

        for msg in msgs {
            match msg {
                GitMsg::Output(line) => {
                    self.git_output.push(line);
                }
                GitMsg::Success(line) => {
                    self.git_output.push(line.clone());
                    self.status_msg = Some(line);
                }
                GitMsg::Failed(line) => {
                    self.git_output.push(format!("ERR: {}", line));
                    self.status_msg = Some(line);
                }
                GitMsg::Done => {
                    self.git_op_active = false;
                    self.git_op_rx = None;
                    self.refresh_git_status();
                    return;
                }
            }
        }
    }

    /// Get summary counts for status bar.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for p in &self.packages {
            match p.status {
                Status::Ok => counts.ok += 1,
                Status::NotInstalled => counts.not_installed += 1,
                Status::BuildReady => counts.build_ready += 1,
                Status::NeedsBuild => counts.needs_build += 1,
                Status::UpdateAvailable => counts.update_avail += 1,
                Status::BuildFailed => counts.build_failed += 1,
            }
        }
        counts
    }
}

#[derive(Default)]
pub struct StatusCounts {
    pub ok: usize,
    pub not_installed: usize,
    pub build_ready: usize,
    pub needs_build: usize,
    pub update_avail: usize,
    pub build_failed: usize,
}
