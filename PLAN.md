# VPM — Void Package Manager TUI

## Context

Managing ~24 custom packages in ~/void-packages (18 Hyprland ecosystem + 6 others) is entirely manual: checking versions, computing build order, rebuilding dependents, syncing git branches. The Hyprland ecosystem has deep interdependencies — bumping `hyprutils` requires rebuilding 15+ packages in the right order. This tool automates tracking and orchestration.

## Reference

- [void-packages Manual](https://github.com/void-linux/void-packages/blob/master/Manual.md) — template format spec, build styles, dependency types
- `~/void-packages/HYPRLAND.md` — current state, blockers, SONAME tracking, build order
- `~/obsidian-vault/system/void-packages.md` — git workflow, package list, maintenance commands

## Approach

Rust TUI using ratatui. Phased delivery — Phase 1+2 is the MVP (read-only dashboard with upstream awareness). Later phases add build orchestration and git workflow.

---

## Phase 1: Project Setup + Template Parser + Version Sources [DONE]

**Goal:** Parse templates, discover packages, and gather version data from all sources.

### Files

```
src/
├── main.rs           # Entry point, arg parsing, `dump` subcommand
├── package.rs         # Template parser + Package/PackageState/Status
├── repo.rs            # Package discovery, xbps-query, built .xbps scanning
└── version_check.rs   # GitHub API + xbps-src update-check + cache
```

### Key decisions made during implementation

- Template parser: line-by-line, handles simple/quoted/multiline assignments, `+=` appends, `$variable` refs, `${version}` substitution
- Package discovery: `git log --name-only --pretty=format: master..custom -- srcpkgs/`, filter symlinks. Note: `git diff` shows all diverged files between branch tips; `git log` shows only files touched by commits on custom.
- Built .xbps scanning: strips `.arch.xbps` suffix, filters subpackages by requiring version starts with digit
- Version checking: GitHub releases API (with tags fallback), xbps-src update-check fallback, JSON cache with 1hr TTL, rustls (no openssl dep)
- Status when not installed but .xbps exists → BuildReady (not NotInstalled)

---

## Phase 2: Dashboard TUI [DONE]

**Goal:** Interactive package list with dependency tree, detail view, and full status pipeline.

### Files

```
src/
├── app.rs         # App state, event handling, views
├── ui.rs          # Rendering (list, tree, detail views)
└── dep_graph.rs   # Dependency graph, topological sort
```

### Status Pipeline

| Priority | Label | Color | Description |
|----------|-------|-------|-------------|
| 1 | **NOT INSTALLED** | yellow | Template exists, no built .xbps, not installed |
| 2 | **UPDATE AVAIL** | peach | Upstream has newer version than template |
| 3 | **NEEDS BUILD** | yellow | Template newer than built .xbps (or no .xbps) |
| 4 | **BUILD READY** | peach | Built .xbps newer than installed (or not installed but .xbps exists) |
| 5 | **OK** | green | Installed matches template, no upstream updates |

### Keybinds

`j/k` navigate, `Enter` detail, `t` tree, `u` check upstream, `U` force-refresh, `r` refresh, `q`/`Ctrl+C` quit, `Esc` back

### Colors — Catppuccin Macchiato

---

## Phase 3: Template Bumping + Build Operations [DONE]

**Goal:** Automatically bump template versions and orchestrate builds.

### Template Version Bump

Keybind: `U` on a package with UPDATE AVAIL status.

**What it does (all package types):**
1. Get latest upstream version (already known from version check)
2. Download new source tarball from updated distfiles URL
3. Compute SHA256 checksum
4. Update template: `version=<new>`, `revision=1`, `checksum=<new>`
5. Queue a build

**This works for all build styles** — the version/checksum bump is purely mechanical regardless of whether the package is a binary repack (zed, google-chrome), cargo build (macchina), or cmake build (hyprland). The build step is where failures surface.

**Special cases:**
- `distfiles` URL with `${version}` substitution: just works, version variable is updated
- Packages with multiple distfiles: update all checksums (rare for custom packages)
- `revision` always resets to 1 on version bump

### Build Operations

### New File: `build.rs`

- Build queue with status (pending/building/success/failed)
- `./xbps-src pkg <name>` via Command with piped stdout
- Build log streaming via `mpsc::channel` + background thread
- Auto-rebuild: queue all reverse dependents in topological order
- Failed builds → store error log in `~/.cache/vpm/build_history.json`, show BUILD FAILED status with reason in detail panel
- **Never runs sudo** — after build, shows: `Run: xi pkg1 pkg2 ...`

### New Status

| Priority | Label | Color | Description |
|----------|-------|-------|-------------|
| 0 | **BUILD FAILED** | red | Build attempted and failed. Error log available. |

### Keybinds

`b` build selected, `B` build with dependents, `U` bump version + build, `Esc` cancel queue

---

## Phase 4: Git Workflow [DONE]

### New File: `git.rs`

- `GitStatus` struct: branch, ahead/behind, last fetch time (from FETCH_HEAD mtime)
- `GitMsg` enum for background thread communication (Output/Success/Failed/Done)
- `GitOp` enum: SyncMaster, RebaseCustom, PushCustom
- SyncMaster: `git fetch void master` + `git update-ref` (no checkout needed)
- RebaseCustom: `git rebase master`, auto-abort on conflict
- PushCustom: `git push origin custom --force-with-lease`
- Streaming output via `run_streaming()` helper (pipes stdout+stderr line by line)

### UI

- Header shows: `custom | 5 ahead | synced 2h ago` (peach for ahead, yellow for behind)
- `g` opens git panel (height 10) with labeled menu
- Panel shows streaming output during operations, full menu after completion
- Keybinds: `1` sync, `2` rebase, `3` push (only when panel open and no op active)

### Also fixed in this phase

- Package discovery: switched from `git diff` to `git log` (diff showed 141 diverged upstream files, log shows only ~26 files from custom commits)
- Package list: added `TableState` for scrollable table via `render_stateful_widget`

---

---

## Phase 5: Polish & Quality of Life

### 5a: Search/Filter

- `/` opens search input, filters package list by name
- `Esc` clears filter and returns to full list
- Filter state shown in header or status bar

### 5b: SONAME Tracking

- Parse `common/shlibs` to know current registered SONAMEs per package
- After build, detect SONAME changes (run `objdump -p` or parse xbps-src output)
- Warn when a SONAME bump would break dependents
- Relevant: hyprgraphics .so.0 → .so.4 broke hyprland/hyprpaper

### 5c: GCC Version Gate

- Detect system GCC version on startup (`gcc -dumpversion`)
- Store known minimum compiler requirements per package (or detect from build errors)
- Warn before building packages that need GCC 15+ (Hyprland 0.53 blocker)

### 5d: Bulk Operations

- "Rebuild all" — queue all custom packages in topological order
- "Update all" — bump + build all packages with UPDATE AVAIL status
- Essential for when GCC 15 lands and the whole ecosystem needs rebuilding

### 5e: `common/shlibs` Auto-Update

- After a successful build with a new SONAME, offer to update `common/shlibs`
- Parse built .xbps to detect shared libraries and their SONAMEs
- Prevents "UNKNOWN PKG PLEASE FIX!" errors on subsequent builds

### 5f: Install Integration

- After successful build, offer to run `xi <packages>` (with confirmation)
- Show the xi command prominently (already done), but add a keybind to execute it
- Needs sudo — spawn in user's terminal or use `pkexec`

### 5g: Build Log Persistence

- Save full build logs to `~/.cache/vpm/logs/<pkg>-<timestamp>.log`
- Detail panel shows path to log file for failed builds
- Keep last N logs per package, prune old ones

### 5h: Config File

- `~/.config/vpm/config.toml` — void-packages path, custom keybinds, etc.
- Removes hardcoded `~/void-packages` path from main.rs

---

## Implementation Order

| Phase | What you get | Status |
|-------|-------------|--------|
| 1 | Template parser, version sources, package discovery | Done |
| 2 | Interactive dashboard with dep tree, full status pipeline | Done |
| 3 | Template bumping + build orchestration with auto-rebuild queue | Done |
| 4 | Git sync/rebase from TUI | Done |
| 5a | Search/filter packages | Planned |
| 5b | SONAME tracking | Planned |
| 5c | GCC version gate | Planned |
| 5d | Bulk operations (rebuild all, update all) | Planned |
| 5e | common/shlibs auto-update | Planned |
| 5f | Install integration (xi keybind) | Planned |
| 5g | Build log persistence | Planned |
| 5h | Config file | Planned |

---

## Technical Notes

- **reqwest on Void**: default features pull openssl-sys which needs openssl-devel. Use `default-features = false, features = ["rustls-tls"]` to avoid the system dependency.
- **.xbps filename parsing**: format is `name-ver_rev.arch.xbps`. Must strip `.arch.xbps` from the end (rfind), not find first dot (version contains dots). Also filter subpackages by checking character after `name-` prefix is a digit.
- **Zed binary layout**: `bin/zed` (CLI launcher) looks for `../libexec/zed-editor` relative to itself. Template must preserve the `bin/` + `libexec/` sibling relationship — can't put both flat in the same dir.
- **Zed distfiles**: the `zed.dev/api/releases/stable/latest/` URL is a redirect to the current release — checksum breaks on upstream updates. Pin to `github.com/zed-industries/zed/releases/download/v${version}/` instead.
- **Status priority when not installed**: if a built .xbps exists but package isn't installed, status should be BUILD READY (actionable) not NOT INSTALLED (implies nothing is built).
