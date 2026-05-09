# AGENTS.md — vxpm

Updated: 2026-05-09

Rust/ratatui TUI for managing the ~24 custom packages in `~/void-packages` (18 Hyprland-ecosystem + 6 others). Tracks versions, checks upstream, computes dependency-aware build order, rebuilds dependents, and drives the git workflow — replaces manual checking when bumping `hyprutils` requires rebuilding 15+ packages. Published as `0x7461/vxpm` on GitHub; xbps-src template at `~/void-packages/srcpkgs/vxpm/template`.

Audience: agents editing this repo. Public-facing feature list + keybinds in `README.md`. Decisions, internals, history in `PLAN.md` (local-only — gitignored).

## Setup

```bash
cargo build --release
```

Rust toolchain via `rustup` (not proto). Requires Void Linux with `xbps-query`, `xbps-src` on `PATH`.

## Commands

```bash
# Build & run
cargo build --release
./target/release/vxpm                    # interactive TUI
./target/release/vxpm dump               # non-interactive package state dump (JSON)

# Tests / lints
cargo test
cargo clippy --all-targets

# Publish a new version (full procedure in PUBLISHING.md)
cargo bump <patch|minor|major>           # then git tag, push, GH Actions builds
```

Config bootstrap: `~/.config/vxpm/config.toml` is auto-created on first run. GCC gate requirements: `~/.config/vxpm/gcc_requirements.toml`.

## Project layout

```
src/
├── main.rs            entry, args, `dump` subcommand
├── app.rs             App state, event handling, views
├── ui.rs              rendering (list / tree / detail)
├── package.rs         template parser, Package/PackageState/Status
├── repo.rs            discovery, xbps-query, built .xbps scanning
├── version_check.rs   GitHub API + xbps-src update-check + cache
├── dep_graph.rs       dependency graph, topological sort
├── build.rs           build queue, streaming logs, auto-rebuild
├── git.rs             sync master / rebase custom / push (streaming)
├── shlibs.rs          SONAME tracking + auto-update of common/shlibs
├── gcc.rs             GCC version gate
└── config.rs          TOML config with ~ expansion
```

External integration points:
- `~/void-packages/` — discovery, build, git ops all target this repo (configurable in `config.toml`).
- `~/.config/vxpm/{config.toml,gcc_requirements.toml}` — user config.
- `~/.cache/vxpm/build_history.json` — persisted build history.
- `~/.cache/vxpm/logs/<pkg>-<ts>.log` and `<pkg>-bump-<ts>.log` — build/bump logs.
- `~/void-packages/hostdir/sources/<filename>` — download cache (avoids double-download with xbps-src).

## Status pipeline

| Priority | Label | Meaning |
|---|---|---|
| 0 | BUILD FAILED | Build attempted, failed. Log available. |
| 1 | NOT INSTALLED | Template exists, no .xbps, not installed. |
| 2 | UPDATE AVAIL | Upstream newer than template. |
| 3 | NEEDS BUILD | Template newer than built .xbps (or no .xbps). |
| 4 | BUILD READY | .xbps newer than installed (or not installed but .xbps exists). |
| 5 | OK | Installed matches template, no upstream updates. |

Badges: `!so` = SONAME mismatch; `GCC N+` = version-gated.

## Boundaries & gotchas

**Always do:**
- **Use `default-features = false, features = ["rustls-tls"]` for `reqwest`.** Default features pull `openssl-sys`; rustls-tls works on Void without system openssl-dev.
- **Stream large downloads, hash in 64KB chunks.** `.bytes()` buffers in memory and times out on tarballs like ollama (~1.9 GB). See `version_check.rs`.
- **Cache downloads to `hostdir/sources/<filename>`.** `download_and_checksum` streams to disk while hashing; if the file is already present, skip the network. Avoids double-download (vxpm + xbps-src).
- **Use `git log --name-only --pretty=format: master..custom -- srcpkgs/`** for package discovery, NOT `git diff`. `diff` shows 141 diverged upstream files; `log` shows only the ~26 touched by custom commits.
- **Strip `.arch.xbps` with `rfind('.')`,** not first dot — versions contain dots.
- **Filter subpackages by checking the character after `name-` is a digit** when scanning built .xbps.
- **Set `pkg_last_checked = SystemTime::now()` directly in `poll_version_check` Done handler,** not from cache. Within TTL (<1h), no disk timestamps update → header appears frozen if read from disk.
- **`git rebase --abort` on cancel or failure during `RebaseCustom`.** `run_git_op` takes `cancel: Arc<AtomicBool>` + `current_child` mutex; rebase cleanup is mandatory.

**Never do:**
- **Never run `sudo` from this binary.** After build, print `Run: xi pkg1 pkg2 ...` and let the user install. Hard rule — sudo elevation is the user's responsibility, not the TUI's.
- **Don't use `env!("HOME")`** — bakes `$HOME` at compile time. Always `std::env::var("HOME")` (matches `config.rs`).
- **Don't use `.read_timeout()` on `reqwest 0.12`** — doesn't exist. `.timeout()` (overall) cuts legitimate large-download transfers; omit the timeout and rely on user-initiated cancel via Esc.
- **Don't call `refresh()` after setting a status message.** `refresh()` clobbers prior messages with `"Refreshed"`. Order: `refresh()` first, then set the real message. For bump failures: save the message and restore after AllDone.

**Ask first:**
- Anything touching `~/void-packages/` outside the configured discovery scope.
- Adding install integration (`xi` keybind). Skipped intentionally; needs sudo.
- Re-tagging a published release. Procedure exists (`gh release delete`, `git tag -d`, push deletion, re-tag, push, recreate) but is destructive — use only for pre-user-impact fixes, otherwise bump version.

**Untested / known-fragile:**
- GitHub Actions–only releases (artifact uploads instead of releases-API binaries) — version checking doesn't detect these. In backlog.

### Cancel / cleanup architecture

Active operations (build / bump / git) all share the same Esc→confirm modal pattern: Esc during op sets `cancel_confirm: Option<String>`; `y` confirms. `q`/Ctrl+C shows `quit_confirm` when any op is running; on confirm → `kill_all()`.

`kill_all()`: `Child` handles in `Arc<Mutex<Option<Child>>>`, shared between background thread and `App`. Build thread stores child after spawn, clears after wait. Git `run_streaming` stores its child too; stdout thread kills on cancel flag.

### Package-specific gotchas

- **Zed binary layout:** `bin/zed` CLI launcher expects `../libexec/zed-editor`. Preserve `bin/` + `libexec/` sibling relationship.
- **Zed distfiles:** `zed.dev/api/releases/stable/latest/` is a redirect → checksum breaks on upstream updates. Pin to `github.com/zed-industries/zed/releases/download/v${version}/`.
- **Zed desktop rename (~v0.224):** `share/applications/zed.desktop` → `dev.zed.Zed.desktop`. Check with `tar -tzf ... | grep desktop` if `do_install` fails.
- **Ollama binary repack:** `ollama-linux-amd64.tar.zst` has no top-level dir — use `create_wrksrc=yes`. CUDA runners (`lib/ollama/cuda_v12`, `cuda_v13`) drag in `libcuda.so.1` (no Void package) → skip in `do_install`, install CPU + Vulkan runners only.

## Workflows

### Publishing a new version

Full step-by-step in [`PLAN.md ## Operations / Publishing`](./PLAN.md#operations--publishing). High-level:
1. Bump version in `Cargo.toml`.
2. `git tag v<x.y.z>`, push tag.
3. GH Actions workflow builds release binary and creates GitHub release. Required: `permissions: contents: write` in workflow (or upload fails with "Resource not accessible by integration").
4. Update xbps-src template at `~/void-packages/srcpkgs/vxpm/template` with new version + checksum.

### `u` vs `U` (version checks)

- `u` (single-select): bypasses cache, always fresh.
- `U` (all): respects 1h TTL. Status bar shows `(cached Xm ago)` for cached results.
- Rationale: users expect a fresh check when pressing the single-select key; bulk check should be cached to avoid GitHub rate limits.

### Re-tagging a release

For pre-user-impact fixes only (use a version bump otherwise):
```bash
gh release delete v<x.y.z>
git tag -d v<x.y.z>
git push origin :refs/tags/v<x.y.z>
git tag v<x.y.z>            # at latest commit
git push origin v<x.y.z>
```

## Where to look

- **`README.md`** — public feature list, full keybind reference, install instructions.
- **`PLAN.md`** (local-only, gitignored) — `## Decisions`, `## Internals`, `## Operations / Publishing`, `## History`.
- **`~/void-packages/HYPRLAND.md`** — current Hyprland ecosystem state, blockers, SONAME tracking.
- **`~/obsidian-vault/system/void-packages.md`** — git workflow + maintenance commands.
