use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use ratatui::widgets::TableState;

use crate::build::{self, BuildHistory, BuildJob, BuildJobStatus, BuildMsg, BuildQueue};
use crate::dep_graph::DepGraph;
use crate::gcc::GccInfo;
use crate::git::{self, GitMsg, GitStatus};
use crate::package::{Package, PackageState, Status};
use crate::repo;
use crate::shlibs::{self, ShlibMap};
use crate::template;
use crate::version_check;

pub enum TemplateBumpMsg {
    Done(String, String, String), // (pkgname, old_version, new_version)
    Failed(String, String),       // (pkgname, error_msg)
    AllDone,
}

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
    Help,
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
    pub shlib_updates: Vec<(String, String, String, String)>, // (pkg, old_so, new_so, new_pkgver)
    pub build_log_scroll: usize, // lines offset from bottom; 0 = follow tail
    pub pkg_last_checked: Option<u64>, // unix timestamp of last pkg upstream check
    pub template_bump_rx: Option<Receiver<TemplateBumpMsg>>,
    pub template_bumping: bool,
}

/// Discover and load all packages (committed + uncommitted).
/// Returns (packages, dep_graph, uncommitted_set).
fn discover_and_load(
    void_pkgs: &std::path::Path,
) -> anyhow::Result<(Vec<crate::package::Package>, DepGraph, std::collections::HashSet<String>)> {
    let committed = repo::discover_custom_packages(void_pkgs)?;
    let committed_set: std::collections::HashSet<String> = committed.iter().cloned().collect();
    let uncommitted = repo::discover_uncommitted_packages(void_pkgs, &committed_set);
    let mut names = committed;
    names.extend(uncommitted.iter().cloned());
    names.sort();
    let uncommitted_set: std::collections::HashSet<String> = uncommitted.into_iter().collect();
    let packages = repo::load_packages(void_pkgs, &names);
    let dep_graph = DepGraph::build(&packages);
    Ok((packages, dep_graph, uncommitted_set))
}

impl App {
    pub fn new(void_pkgs: PathBuf) -> anyhow::Result<Self> {
        let (packages, dep_graph, uncommitted_set) = discover_and_load(&void_pkgs)?;
        let mut states = repo::build_package_states(&void_pkgs, packages, &uncommitted_set);

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

        // Prune old build logs at startup
        build::prune_build_logs(5);

        Ok(App {
            packages: states,
            selected: 0,
            view: View::List,
            dep_graph,
            void_pkgs,
            panel: PanelMode::None,
            checking_versions: false,
            status_msg: None,
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
            shlib_updates: Vec::new(),
            build_log_scroll: 0,
            pkg_last_checked: version_check::last_check_time(),
            template_bump_rx: None,
            template_bumping: false,
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
        if let Ok((packages, dep_graph, uncommitted_set)) = discover_and_load(&self.void_pkgs) {
            self.dep_graph = dep_graph;
            let mut states = repo::build_package_states(&self.void_pkgs, packages, &uncommitted_set);

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

    /// Check upstream version for the selected package only.
    pub fn check_version_selected(&mut self) {
        if self.checking_versions {
            self.status_msg = Some("Version check already in progress".to_string());
            return;
        }
        let pkg = match self.selected_package() {
            Some(p) => p.package.clone(),
            None => return,
        };
        self.checking_versions = true;
        self.status_msg = Some(format!("Checking upstream for {}...", pkg.name));
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.version_check_rx = Some(rx);
        std::thread::spawn(move || {
            version_check::check_all_versions_streaming(&void_pkgs, &[pkg], false, tx);
        });
    }

    /// Check upstream versions for all packages.
    pub fn check_versions(&mut self) {
        if self.checking_versions {
            return;
        }
        self.checking_versions = true;
        self.status_msg = Some("Checking upstream versions...".to_string());

        let pkgs: Vec<Package> = self.packages.iter().map(|s| s.package.clone()).collect();
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.version_check_rx = Some(rx);

        std::thread::spawn(move || {
            version_check::check_all_versions_streaming(&void_pkgs, &pkgs, false, tx);
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
                    // Use current time — cache TTL hits don't update disk timestamps
                    // but the user did just run a check, so "last checked" = now.
                    self.pkg_last_checked = Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    );
                    self.status_msg = Some(format!("Checked {} packages", count));
                    return;
                }
            }
        }
    }

    /// Poll for template bump results. Call each tick.
    pub fn poll_template_bump(&mut self) {
        let msgs: Vec<TemplateBumpMsg> = if let Some(ref rx) = self.template_bump_rx {
            rx.try_iter().collect()
        } else {
            return;
        };

        for msg in msgs {
            match msg {
                TemplateBumpMsg::Done(name, old, new) => {
                    self.status_msg = Some(format!("Bumped {} {} → {}", name, old, new));
                }
                TemplateBumpMsg::Failed(name, err) => {
                    self.status_msg = Some(format!("Bump failed for {}: {}", name, err));
                }
                TemplateBumpMsg::AllDone => {
                    self.template_bumping = false;
                    self.template_bump_rx = None;
                    self.refresh();
                    return;
                }
            }
        }
    }

    /// Bump template for the selected package (UpstreamAhead only). Does not build.
    pub fn bump_template_selected(&mut self) {
        if self.template_bumping {
            self.status_msg = Some("Template bump already in progress".to_string());
            return;
        }
        let (name, latest) = match self.selected_package() {
            Some(p) if p.status == Status::UpstreamAhead => match p.latest.clone() {
                Some(v) => (p.package.name.clone(), v),
                None => {
                    self.status_msg = Some("No upstream version known — run u first".to_string());
                    return;
                }
            },
            Some(p) => {
                self.status_msg = Some(format!("{} — not upstream ahead", p.package.name));
                return;
            }
            None => return,
        };
        self.template_bumping = true;
        self.status_msg = Some(format!("Bumping {} to v{}...", name, latest));
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.template_bump_rx = Some(rx);
        std::thread::spawn(move || {
            match template::bump_template(&void_pkgs, &name, &latest) {
                Ok(result) => {
                    let _ = tx.send(TemplateBumpMsg::Done(name, result.old_version, result.new_version));
                }
                Err(e) => {
                    let _ = tx.send(TemplateBumpMsg::Failed(name, e.to_string()));
                }
            }
            let _ = tx.send(TemplateBumpMsg::AllDone);
        });
    }

    /// Bump templates for all UpstreamAhead packages. Does not build.
    pub fn bump_template_all(&mut self) {
        if self.template_bumping {
            self.status_msg = Some("Template bump already in progress".to_string());
            return;
        }
        let targets: Vec<(String, String)> = self
            .packages
            .iter()
            .filter(|p| p.status == Status::UpstreamAhead)
            .filter_map(|p| p.latest.as_ref().map(|v| (p.package.name.clone(), v.clone())))
            .collect();
        if targets.is_empty() {
            self.status_msg = Some("No packages with upstream updates".to_string());
            return;
        }
        self.template_bumping = true;
        self.status_msg = Some(format!("Bumping {} packages...", targets.len()));
        let void_pkgs = self.void_pkgs.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.template_bump_rx = Some(rx);
        std::thread::spawn(move || {
            for (name, latest) in targets {
                match template::bump_template(&void_pkgs, &name, &latest) {
                    Ok(result) => {
                        let _ = tx.send(TemplateBumpMsg::Done(
                            name,
                            result.old_version,
                            result.new_version,
                        ));
                    }
                    Err(e) => {
                        let _ = tx.send(TemplateBumpMsg::Failed(name, e.to_string()));
                    }
                }
            }
            let _ = tx.send(TemplateBumpMsg::AllDone);
        });
    }

    /// Build the poll queue for messages from the background thread.
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
                    self.build_log_scroll = 0;
                    self.status_msg = Some(format!("Building {}...", name));
                }
                BuildMsg::Output(_name, line) => {
                    self.build_queue.current_output.push(line);
                    // Trim deferred to after the loop — O(1) amortized vs O(n²) drain per msg
                }
                BuildMsg::Finished(name, log_path) => {
                    for job in &mut self.build_queue.jobs {
                        if job.name == name {
                            job.status = BuildJobStatus::Success;
                        }
                    }
                    // Store log path and check for shlib updates
                    let log_str = log_path.to_string_lossy().to_string();
                    for state in &mut self.packages {
                        if state.package.name == name {
                            state.build_log = Some(log_str.clone());
                            // Check for pending shlib updates
                            for mm in &state.soname_mismatches {
                                let pkg_ver = format!(
                                    "{}-{}_{}",
                                    state.package.name, state.package.version, state.package.revision
                                );
                                self.shlib_updates.push((
                                    state.package.name.clone(),
                                    mm.registered.clone(),
                                    mm.installed.clone(),
                                    pkg_ver,
                                ));
                            }
                        }
                    }
                    self.build_history.record(&name, true);
                }
                BuildMsg::Failed(name, error_lines, log_path) => {
                    for job in &mut self.build_queue.jobs {
                        if job.name == name {
                            job.status = BuildJobStatus::Failed;
                        }
                    }
                    // Set BuildFailed status and store log path
                    let log_str = log_path.to_string_lossy().to_string();
                    for state in &mut self.packages {
                        if state.package.name == name {
                            state.status = Status::BuildFailed;
                            state.build_log = Some(log_str.clone());
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

        // Trim output buffer once after processing all messages (avoids O(n²) drain per message).
        let out = &mut self.build_queue.current_output;
        if out.len() > 200 {
            out.drain(..out.len() - 200);
        }
    }

    /// Build the currently selected package (best-effort: skip if not buildable).
    pub fn build_selected(&mut self) {
        if self.build_queue.active {
            self.status_msg = Some("Build already in progress".to_string());
            return;
        }

        let (name, status) = match self.selected_package() {
            Some(p) => (p.package.name.clone(), p.status.clone()),
            None => return,
        };

        match status {
            Status::BuildOutdated | Status::BuildFailed => {}
            Status::UpstreamAhead => {
                self.status_msg = Some(format!("{} — upstream ahead, bump template first (t)", name));
                return;
            }
            Status::ReadyToInstall => {
                self.status_msg = Some(format!("{} — already built, run: xi {}", name, name));
                return;
            }
            Status::UpToDate => {
                self.status_msg = Some(format!("{} — nothing to build", name));
                return;
            }
        }

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

/// Build all packages with BuildOutdated or BuildFailed status in topo order.
    pub fn build_all_buildable(&mut self) {
        if self.build_queue.active {
            self.status_msg = Some("Build already in progress".to_string());
            return;
        }

        let buildable: std::collections::HashSet<String> = self
            .packages
            .iter()
            .filter(|p| matches!(p.status, Status::BuildOutdated | Status::BuildFailed))
            .filter(|p| !self.gcc_info.is_blocked(&p.package.name))
            .map(|p| p.package.name.clone())
            .collect();

        let blocked_count = self
            .packages
            .iter()
            .filter(|p| matches!(p.status, Status::BuildOutdated | Status::BuildFailed))
            .filter(|p| self.gcc_info.is_blocked(&p.package.name))
            .count();

        if buildable.is_empty() {
            let hint = if blocked_count > 0 {
                format!("No buildable packages ({} GCC-blocked)", blocked_count)
            } else {
                "No packages to build (try 't' to bump templates)".to_string()
            };
            self.status_msg = Some(hint);
            return;
        }

        let topo = self.dep_graph.topological_sort();
        let ordered: Vec<String> = topo.into_iter().filter(|n| buildable.contains(n)).collect();

        let msg = if blocked_count > 0 {
            format!("Building {} packages ({} GCC-blocked, skipped)...", ordered.len(), blocked_count)
        } else {
            format!("Building {} packages...", ordered.len())
        };
        self.status_msg = Some(msg);

        self.build_queue.jobs = ordered
            .into_iter()
            .map(|n| BuildJob { name: n, status: BuildJobStatus::Pending })
            .collect();
        self.build_queue.start(self.void_pkgs.clone());
        self.panel = PanelMode::BuildLog;
    }

/// Apply pending shlib updates to common/shlibs.
    pub fn apply_shlib_updates(&mut self) {
        if self.shlib_updates.is_empty() {
            return;
        }

        let updates: Vec<(String, String, String)> = self
            .shlib_updates
            .iter()
            .map(|(_, old, new, pkgver)| (old.clone(), new.clone(), pkgver.clone()))
            .collect();

        match shlibs::update_shlibs_file(&self.void_pkgs, &updates) {
            Ok(()) => {
                let count = self.shlib_updates.len();
                self.shlib_updates.clear();
                self.refresh();
                self.status_msg = Some(format!("Updated {} shlib entries", count));
            }
            Err(e) => {
                self.status_msg = Some(format!("Failed to write common/shlibs: {}", e));
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

    pub fn scroll_log_up(&mut self) {
        let max = self.build_queue.current_output.len().saturating_sub(1);
        self.build_log_scroll = (self.build_log_scroll + 1).min(max);
    }

    pub fn scroll_log_down(&mut self) {
        self.build_log_scroll = self.build_log_scroll.saturating_sub(1);
    }

    /// Packages with no known upstream version yet.
    pub fn unchecked_count(&self) -> usize {
        self.packages.iter().filter(|p| p.latest.is_none()).count()
    }

    /// Get summary counts for status bar.
    pub fn status_counts(&self) -> StatusCounts {
        let mut counts = StatusCounts::default();
        for p in &self.packages {
            match p.status {
                Status::UpToDate => counts.up_to_date += 1,
                Status::UpstreamAhead => counts.upstream_ahead += 1,
                Status::BuildOutdated => counts.build_outdated += 1,
                Status::ReadyToInstall => counts.ready_to_install += 1,
                Status::BuildFailed => counts.build_failed += 1,
            }
        }
        counts
    }
}

#[derive(Default)]
pub struct StatusCounts {
    pub up_to_date: usize,
    pub upstream_ahead: usize,
    pub build_outdated: usize,
    pub ready_to_install: usize,
    pub build_failed: usize,
}
