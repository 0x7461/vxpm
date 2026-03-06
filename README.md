# vxpm

A terminal UI for managing custom [void-packages](https://github.com/void-linux/void-packages) templates.

Built for the workflow of maintaining self-built packages on Void Linux — track versions, check upstream updates, rebuild dependencies in order, and manage the git workflow, all from a single TUI.

> Developed with [Claude Code](https://claude.ai/claude-code).

## Features

- Dashboard of all custom packages with live status (`UP TO DATE`, `UPSTREAM AHEAD`, `BUILD OUTDATED`, `READY TO INSTALL`)
- Upstream version checking against GitHub releases/tags
- One-key template bump + build
- Dependency-aware build ordering
- SONAME mismatch detection and `common/shlibs` auto-update
- GCC version gate tracking
- Build log viewer with scrolling
- Git integration (ahead/behind count, commit workflow)
- Uncommitted template detection

## Keybinds

| Key | Action |
|-----|--------|
| `j`/`k` | Navigate |
| `/` | Search/filter |
| `u` | Check upstream versions (selected) |
| `U` | Check upstream versions (all) |
| `t` | Bump template to latest (selected) |
| `T` | Bump all upstream-ahead templates |
| `b` | Build selected |
| `B` | Build all outdated |
| `g` | Git status |
| `s` | Apply SONAME shlib updates |
| `Enter` | Detail panel |
| `?` | Help |
| `q` | Quit |

## Install

Download the binary from [releases](https://github.com/0x7461/vxpm/releases) or build from source:

```sh
cargo build --release
# Binary at target/release/vxpm
```

## Requirements

- Void Linux with [void-packages](https://github.com/void-linux/void-packages) checked out
- `xbps-query`, `xbps-src` in PATH
- Config: `~/.config/vxpm/config.toml` (auto-created on first run)

## License

MIT
