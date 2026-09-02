# CLAUDE.md

Guidance for working in this repo (a Rust herdr plugin: credential-scoped AI
quota and context in Herdr's sidebar for Claude, Codex, Grok, Agy, OpenCode,
Pi, and omp).

For the plugin's own design rules, event-path budget, and Herdr-integration
gotchas, read `AGENTS.md` first — it is the working method and is not
duplicated here. This file covers the Windows port specifically.

## Remotes

- `origin` (`henriquecalhohicx/herdr-agent-quota`) is the personal fork and
  the **only** push target — branch `windows-port` is the working branch.
- `upstream` (`levi-qiao/herdr-agent-quota`) is **read-only reference**: fetch
  works, do not push, do not open PRs against it, do not merge
  `upstream/main` without a specific reason — same rationale as the sibling
  `herdr-cache-ttl` fork's `origin`/`upstream` split in this workspace.

## Build / test (Windows dev)

- CI gate (must stay green, per `CONTRIBUTING.md`): `cargo fmt --all --
  --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all-targets --all-features --locked`. All three pass on this
  machine (Rust 1.95.0, MSVC toolchain, `herdr 0.8.2-preview.2026-08-31`) as
  of this port.
- `cargo build --release` produces `target\release\herdr-agent-quota.exe`.
  The `[[build]]` manifest entry (`cargo build --release`) is a plain argv
  command, not a shell script, so it already runs unmodified on Windows — it
  did not need a `-win` twin, unlike every startup/action/event/pane entry.

## Cross-platform rules

- Linux/macos behavior stays byte-identical. Windows code is additive behind
  `#[cfg(windows)]`; the prior Unix-only code is now behind `#[cfg(unix)]`
  where it previously compiled unconditionally. `libc` is
  `[target.'cfg(unix)']`; `windows-sys` 0.59 (pinned to match the sibling
  `herdr-cache-ttl` fork) is `[target.'cfg(windows)']`, with exactly the
  feature flags actually referenced: `Win32_Foundation`,
  `Win32_Storage_FileSystem`, `Win32_System_Pipes`, `Win32_System_JobObjects`,
  `Win32_System_Threading`, `Win32_System_IO`, `Win32_Security` (the last four
  were required transitively by `windows-sys` 0.59's own feature gating for
  `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, `CreateFileW`/`ReadFile`/`WriteFile`,
  and `OVERLAPPED` — narrower feature sets failed to compile; see the build
  log this port went through if trimming further).
- herdr rejects duplicate action/pane ids across platform variants even with
  disjoint `platforms` (confirmed independently in the sibling
  `herdr-cache-ttl` and `herdr-pc-ram-and-cpu-usage-overlay` forks) — every
  Windows action/pane/event variant added here uses a `-win`-suffixed id.

## What this port covers

Three Unix-only surfaces, each ported behind `#[cfg(windows)]` next to the
existing `#[cfg(unix)]` code:

1. **`src/herdr.rs::socket_request`** — the raw `HERDR_SOCKET_PATH` request
   used only by `agent.view.set`/`agent.view.clear` (the `--agent-order
   quota` sidebar sort; `agent.view.*` has no CLI subcommand in Herdr 0.8).
   Windows counterpart opens the named pipe `\\.\pipe\<HERDR_SOCKET_PATH>`
   via `CreateFileW`, writes one NDJSON line via `WriteFile`, and reads one
   line back via `ReadFile` — unlike the sibling `herdr-cache-ttl` fork's
   `socket_send` (fire-and-forget, write-only), this one has to read a reply,
   so it is not a simple copy of that fork's technique.
   - **Known limitation — no read/write timeout.** The unix path sets
     `set_read_timeout`/`set_write_timeout` (`SOCKET_TIMEOUT`, 5s) on the
     `UnixStream`. The Windows `CreateFileW`/`ReadFile`/`WriteFile` calls here
     use a synchronous (non-overlapped) handle, which has no equivalent
     timeout — a stalled or wedged Herdr server could block this call
     indefinitely instead of failing after 5 seconds. A real timeout needs
     overlapped I/O (`OVERLAPPED` + `WaitForSingleObject`), which is a bigger
     change than this pass attempts.
2. **`src/process.rs`** — `run_shell_with_deadline`'s process-group kill
   (`libc::setpgid`/`killpg`) for a timed-out previous-statusLine shell
   command. Windows counterpart is a Job Object (`WindowsJob`,
   `CreateJobObjectW` + `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` +
   `AssignProcessToJobObject` + `TerminateJobObject`).
   - **Race window, by design, not a bug to fix.** Unix's `setpgid` runs
     inside the child via `pre_exec`, before it execs anything else — the
     child is in its own process group from its very first instruction.
     `std::process::Command` on Windows has no `pre_exec` equivalent, so the
     job can only be created and the child assigned to it *after* `spawn()`
     returns. If that child immediately spawned a helper of its own inside
     that window, the helper could start outside the job and survive a
     `terminate()`. Accepted as a known limitation (documented at the
     `WindowsJob` struct) rather than engineering a suspended-start dance —
     this code path only ever runs a previous statusLine shell command, which
     in practice does not fork a long-lived helper in that window.
3. **`src/refresh.rs::spawn_watch`** — detaching the active-turn quota
   watcher from the short-lived Herdr event process that starts it
   (`libc::setsid`). Windows counterpart is
   `CommandExt::creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)`
   — a stdlib call, no `windows-sys` needed for this one (the flag values are
   spelled out numerically in `refresh.rs` rather than imported, per the
   task's own note that this needs no extra crate). `reexec_watch`'s
   `#[cfg(not(unix))]` branch (spawn instead of exec, no detach) already
   existed before this port and was left unchanged.
4. **`src/providers/codex.rs::terminate`** — the Codex `app-server` subprocess
   spawned by `fetch_for_sessions` had the identical `setpgid`/`killpg`
   process-group-kill shape as item 2, minus a Windows counterpart, when this
   port first landed (see the removed bullet under "Not touched", below —
   this was picked up in a follow-up pass). `process.rs`'s `WindowsJob` was
   made `pub(crate)` and reused here verbatim: the job is created and the
   app-server child assigned to it right after `spawn()`, and both the
   watchdog thread and the main fetch path call the Windows `terminate`
   overload (`job.terminate()` then `child.kill()`) instead of the unix one
   (`killpg` then `child.kill()`). Same race-window caveat as item 2 applies
   here for the same reason (`Command` has no Windows `pre_exec`).

## Verification actually performed on this Windows machine

- `cargo build --release`, `cargo test --all-targets --all-features
  --locked`, `cargo fmt --all -- --check`, and `cargo clippy --all-targets
  --all-features -- -D warnings` (plus `cargo clippy --release`, per
  `AGENTS.md`) all pass. 328 lib tests + 27 integration tests green.
- **`process.rs`'s Job Object path is genuinely exercised, not just
  compiled.** `windows_job_assigns_and_terminates_a_real_child` (new test)
  spawns a real `cmd /C ping -n 5 127.0.0.1` child, creates a `WindowsJob`,
  assigns the child to it, calls `terminate()`, and asserts the child's exit
  status shows it was killed mid-flight rather than completing on its own —
  proving `CreateJobObjectW` → `SetInformationJobObject` →
  `AssignProcessToJobObject` → `TerminateJobObject` all succeed end-to-end
  against a real Win32 process. The pre-existing
  `kills_a_command_that_exceeds_its_budget` test (`sh -c "sleep 20 & wait"`
  with a 100ms budget) also passes, but **does not by itself prove the
  job-object-specific guarantee** — it would pass identically even if the Job
  Object silently did nothing, because `terminate_and_reap` always falls
  through to a plain `child.kill()` on the immediate `sh` process regardless
  of whether `job` is `Some` or `None`. Neither test spawns a grandchild
  process specifically to check it survives a plain `kill()` but dies with
  the job — that would be the fully conclusive proof and was judged out of
  scope for this pass's effort budget.
- **`codex.rs::terminate`'s reuse of `WindowsJob` is not independently
  exercised** the way `process.rs`'s is. There is no test that spawns a real
  `codex app-server`-shaped child, assigns it to a job, and confirms
  termination — the existing Codex provider tests all run against fixture
  JSON, not a real subprocess. It is the same `WindowsJob` type the direct
  test above proves works end-to-end, but the specific wiring in
  `fetch_for_sessions`/`terminate` (job creation right after
  `command.spawn()`, watchdog-thread `Arc` plumbing) has only been proven by
  `cargo build`/`cargo test` passing, not by an actual kill observed on this
  machine.
- **The named-pipe `socket_request` path has NOT been tested against a live
  Herdr server.** Task instructions for this port explicitly excluded running
  `herdr plugin link` or touching this machine's `plugins.json` — that step
  is left for the user to do separately. So unlike the `WindowsJob` test
  above, `socket_request`'s Windows branch is **compiled but unexercised**
  beyond its own pure unit tests (`windows_pipe_name_prepends_the_pipe_prefix
  _to_a_filesystem_path`, `windows_pipe_name_leaves_an_existing_pipe_path
  _untouched`), which only check the path-prefix string logic — they do not
  open a real pipe. The `CreateFileW`/`WriteFile`/`ReadFile`/`WaitNamedPipeW`
  sequence itself has not been run against anything, live or stubbed. The
  sibling `herdr-cache-ttl` fork's live smoke test (isolated named Herdr
  session, `herdr --session <name> server`) is the template for whoever does
  this verification next; see that repo's `CLAUDE.md` for the exact recipe.
- `spawn_watch`'s `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` flags compile
  and the function is exercised indirectly (any test that reaches
  `refresh::event()` with a `working` status calls `spawn_watch`), but no
  test specifically confirms the spawned watcher process actually detaches
  correctly on Windows (survives its parent event process exiting, does not
  receive Ctrl+C meant for a console it no longer has). Not independently
  verified.
- The Windows `[[startup]]`/`[[actions]]`/`[[events]]`/`[[panes]]` manifest
  entries (`herdr-plugin.toml`) parse as valid TOML (checked with Python's
  `tomllib`) and the existing `tests/plugin_manifest.rs` suite — which
  string-matches manifest content — still passes unchanged. **None of these
  entries has been invoked by a live Herdr install** (again, `herdr plugin
  link` was explicitly out of scope for this pass): the PowerShell launcher
  wrapper convention is copied verbatim from `herdr-cache-ttl`'s
  already-live-verified pattern, but this repo's own copy of it is unverified
  end-to-end.

## Extended beyond the task's literal three-item list

The task that drove this port named `herdr.rs`, `process.rs`, and
`refresh.rs` as the three Windows-blocking surfaces, and described manifest
changes for `[[build]]`, `[[startup]]`, and `[[actions]]` specifically. Two
things were extended past that literal scope because leaving them out would
have shipped a non-functional or non-compiling port:

- **`[[events]]` and `[[panes]]` also got Windows twins.** This plugin has
  three `sh -c`-wrapped events (`pane.agent_detected`,
  `pane.agent_status_changed`, `pane.focused`) and two `sh -c`-wrapped panes
  (`dashboard`, `settings`) that the task's instructions did not explicitly
  mention adding Windows variants for. Leaving them unscoped would have meant
  Herdr tries to run `sh -c "..."` on Windows for all of them — the plugin's
  three event hooks and both UI panes would simply not work there, which
  defeats the point of a Windows port. Ported using the exact same
  PowerShell-launcher convention as the explicitly-requested entries.
- **Two pre-existing tests and one integration test file needed
  platform-gating to get `cargo test` green on Windows**, none of which are
  among the three Unix-only *source* surfaces the task named (all three were
  in `tests/` or test-only code, not production logic):
  - `tests/configure_round_trip.rs` is gated `#![cfg(unix)]` entirely: it
    drives fake `herdr`/statusLine binaries by writing `#!/bin/sh` scripts and
    executing them directly, which relies on the OS honoring the shebang
    line — Windows' `CreateProcess` does not do this. Porting these stubs to
    something Windows can execute directly (a `.cmd`/PowerShell script) is a
    considerably larger effort than this pass's three items and was not
    attempted.
  - `providers::omp::tests::the_cli_is_called_for_one_provider_and_its_report
    _is_parsed` is gated `#[cfg(unix)]` for the same shebang-stub reason.
  - `opencode::tests::database_opens_under_a_path_containing_uri_punctuation`
    is gated `#[cfg(unix)]`: it creates a directory literally named
    `we?ird#dir`, and `?` is invalid in an NTFS filename component, so
    `create_dir_all` itself fails on Windows before the URI-escaping logic
    under test ever runs.
  - `tests/statusline_config.rs`'s
    `repair_migrates_a_previous_backup_from_the_old_state_directory` had a
    genuine test bug (not gated, fixed instead): it hand-formatted a JSON
    fixture by interpolating `PathBuf::display()` directly into a string
    literal. On Windows that path contains backslashes, which are JSON
    escape characters, producing invalid JSON (`invalid escape at line 1
    column 71`) and an unrelated test failure. Fixed by building the fixture
    with `serde_json::json!`/`to_vec` instead of manual string formatting, so
    the path is escaped correctly regardless of platform. No production code
    changed.

## Not touched, and known to still assume Unix in ways unrelated to this port

Found while grepping the full `src/` tree per the task's own instruction to
verify beyond the three named surfaces, but explicitly left alone as outside
this pass's scope (each is a pre-existing runtime-correctness gap, not a
compile blocker — the crate already builds and its own tests already pass on
Windows without touching any of these):

- **Six call sites unconditionally read the `HOME` environment variable**
  and `.context("HOME is not set")` if it is absent:
  `src/providers/codex.rs::codex_home`, `src/providers/grok.rs`,
  `src/opencode.rs` (×2), `src/configure/grok.rs`,
  `src/configure/herdr.rs`, `src/configure/statusline.rs`. A stock Windows
  shell (`cmd.exe`, native PowerShell without profile customization) does not
  set `HOME` — it sets `USERPROFILE` instead — so Codex quota fetching and
  several `configure`/`opencode` code paths would fail with "HOME is not
  set" on a Windows machine unless the user's shell happens to export `HOME`
  (e.g. Git Bash environments typically do). `src/omp.rs` and `src/pi.rs`
  already use `directories::BaseDirs::new()?.home_dir()`, which resolves
  correctly on both platforms — extending the other six call sites to do the
  same (or a `HOME`-then-`USERPROFILE` fallback) is the fix, but it touches
  production logic well outside this pass's three named surfaces and was not
  attempted.

## Layout

- `src/herdr.rs` — Herdr CLI invocations (`Command::new(herdr_bin())`) plus
  the one raw-socket call (`socket_request`, unix domain socket / windows
  named pipe) used only by the Agent-view sort feature.
- `src/process.rs` — `run_shell_with_deadline`, the bounded runner for a
  previous statusLine command, with its process-group kill (`killpg` on
  unix, a `WindowsJob` on windows).
- `src/refresh.rs` — the refresh/publish pipeline and the detached
  active-turn quota watcher (`spawn_watch`/`reexec_watch`).
- `src/providers/` — one file per quota source; `codex.rs` and `grok.rs` read
  local credentials and call the provider directly, `claude.rs`/`agy.rs`
  collect from a statusLine hook, `omp.rs` shells out to `omp usage --json`.
- `src/configure/` — the reversible `configure --apply`/`--uninstall` file
  edits (Claude/Agy statusLine wiring, Grok hook, Herdr sidebar rows).
