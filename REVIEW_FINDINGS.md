# vpm Code Review Findings
> From session cf8a5cc4, 2026-03-06. Agent completed full review.
> All bugs and warnings addressed by 2026-03-08. Suggestions #23/#24/#26 intentionally left (documented/low-risk).

## Bugs (fix first)

1. **`env!("HOME")` at compile time** — `build.rs:80,118`, `version_check.rs:39`
   `env!("HOME")` bakes `$HOME` at compile time. Use `std::env::var("HOME")` consistently (like `config.rs` already does).

2. **Stderr deadlock in build thread** — `build.rs:257-273`
   Reads stdout to completion while stderr is ignored. If xbps-src writes >64KB to stderr, the child blocks writing stderr while parent blocks reading stdout → deadlock. Fix: read stdout+stderr concurrently (like `git.rs:run_streaming` already does).

3. **Lexicographic version comparison for .xbps files** — `repo.rs:157`
   `"9.0_1" > "10.0_1"` lexicographically. Picks wrong "best" version when multiple builds exist across digit boundaries. Use the existing `version_newer` function or proper version parsing.

## Warnings (likely issues in edge cases)

4. **`build.rs:257-273`** — stderr warnings silently lost on successful builds (separate from deadlock).

5. **`package.rs:228-230`** — operator precedence in template parser skip logic. Logic is coincidentally correct but confusing — wrap in parens to make intent explicit.

6. **`package.rs:240-252`** — multiline `+=` append corrupts vars map with spurious space prefix. Masked by downstream `split_whitespace()`.

7. **`repo.rs:157`** — lexicographic version comparison (see Bug #3).

8. **`template.rs:120-121`** — `$var` replacement without braces can match substrings of other variable names. HashMap iteration is random → non-deterministic substitution if a var name is a prefix of another.

9. **`version_check.rs:101-137`** — GitHub API rate limit (60 req/hr unauthenticated). With ~24 packages × 2 requests = 48 req/cycle. Silent failure with no user feedback when rate limited.

10. **`version_check.rs:183-230`** — cache race condition (guarded at app layer by `checking_versions` flag, but not thread-safe at cache layer). Document invariant or add file locking.

11. **`shlibs.rs:60-109`** — spawns `readelf` for every `.so` file at startup, synchronously on main thread. Performance issue for large package sets.

12. **`shlibs.rs:186`** — silent failure on write. `apply_shlib_updates` reports "Updated N entries" even if the file wasn't written (disk full, permissions).

13. **`version_check.rs:39`** — duplicate of Bug #1 (`env!("HOME")` in `dirs_cache()`).

14. **`package.rs:140-158`** — version comparison loses prerelease suffixes (`beta1` → 0). `0.45.0` and `0.45.0beta1` compare as equal → missed updates on release.

15. **`template.rs:160`** — checksum rewrite only handles single-line checksums. Multi-distfile templates (multiple checksums) will be corrupted. Low risk for current package set.

## Suggestions (code quality)

16. **`#[allow(dead_code)]`** — violates CLAUDE.md rule. Remove and delete dead code:
    - `package.rs:60-69` — `Status::priority()`
    - `shlibs.rs:8` — `ShlibEntry::pkg_ver`
    - `template.rs:7` — `BumpResult.new_checksum` field

17. **`build.rs:122-157`** — hand-rolled date/time algo. `chrono` is already a dep (`ui.rs` uses it). Replace `chrono_timestamp()` + `days_to_ymd()` with `chrono::Local::now().format(...)`. Removes ~35 lines.

18. **`app.rs:475-477`** — `Vec::drain(..n)` inside per-message loop → O(n²) for builds producing many output lines. Move drain outside message loop or use `VecDeque`.

19. **`ui.rs:181,189`** — `visible_packages()` called 3+ times per frame. Compute once, pass through.

20. **`version_check.rs:191-225`** — `Done(count)` reports packages with versions found, not attempted. Shows "Checked 14 packages" when 10 failed silently. Track attempted vs found separately.

21. **`template.rs:147-173`** — `rewrite_template` strips quotes from `version="1.0.0"`. Void templates rarely quote version, so low risk.

22. **`app.rs:199-261`** — package discovery logic duplicated between `App::new()` and `App::refresh()`. Extract to helper.

23. **`git.rs:37`** — `master...custom` branch names hardcoded. Intentional per workflow but worth documenting.

24. **`build.rs:176-179`** — log filename parsing strips 16 chars for timestamp. Works for current format but brittle if format changes. OK for now.

25. **`app.rs:86-89`** — `visible_packages` index logic consistent. OK.

26. **`build.rs:240-311`** — cancelled build still sends `QueueComplete`. Fine behavior (succeeded list filters correctly).

27. **`version_check.rs:191-225`** — count only incremented for successful checks (see Suggestion #20).

28. **`app.rs:488-503`** — shlib updates accumulate across builds. Duplicates are idempotent but messy. Deduplicate or clear before adding.
