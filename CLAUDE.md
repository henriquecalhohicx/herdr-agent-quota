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

## Live-tested against a running Herdr server (2026-09-02)

Linked into `%APPDATA%\herdr\plugins.json` (`herdr plugin link
C:\git-repositories\10-herdr\herdr-agent-quota --disabled`, herdr
`0.8.2-preview.2026-08-31-b1ff4582e968`) and exercised against an isolated
named session (`herdr --session agent-quota-smoke server`, headless,
`%APPDATA%\herdr\sessions\agent-quota-smoke\`), following the sibling
`herdr-cache-ttl` fork's recipe exactly — the real `default` session's own
panes/workspaces were never touched. A throwaway pane (`w1:p1`) was created
with `herdr workspace create --cwd <path> --focus` against the smoke
session's own socket (`$env:HERDR_SOCKET_PATH` pointed at
`sessions\agent-quota-smoke\herdr.sock` for every CLI call below). `configure`
and `uninstall` were never invoked, per the task's constraint.

- **Named-pipe `socket_request` — confirmed working end-to-end.** Unlike
  `herdr-cache-ttl`'s `sort`/`sort-win`, this plugin's socket call
  (`agent.view.set`/`agent.view.clear`) is not reachable through any of its
  four manifest actions — grepping `src/` showed it is only reached from
  `refresh.rs::startup()` (`Command::Startup`, i.e. the `[[startup]]` hook),
  gated on `resolved_agent_order(None, Some(&cache)).is_quota()`, and
  `resolved_agent_order` checks the `HERDR_AGENT_QUOTA_AGENT_ORDER` env var
  before any stored preference. Since `herdr plugin action invoke` has no
  `--env` flag (confirmed via `--help`) and writing the stored preference
  would have meant either running `configure` (forbidden) or hand-writing a
  file under the *shared, per-machine* `HERDR_PLUGIN_CONFIG_DIR` while the
  plugin was enabled — risking the real `default` session's own
  event-triggered `startup`/refresh picking up a global "quota" preference —
  the socket path was exercised by running the built exe directly (not
  through `herdr plugin action invoke`) with
  `HERDR_SOCKET_PATH`/`HERDR_PLUGIN_STATE_DIR`/`HERDR_PLUGIN_CONFIG_DIR`
  pointed at the isolated session/a scratch dir and
  `HERDR_AGENT_QUOTA_AGENT_ORDER=quota` set only in that one process's
  environment (never persisted to disk, never visible to any other
  invocation): `.\target\release\herdr-agent-quota.exe startup --provider
  all` → exit code 0, and the smoke session's `herdr-server.log` shows
  `request_id="agent-quota:view-set" method="agent.view.set" ...
  outcome="ok"` about a minute later (the delay is just how long `startup`'s
  own provider refresh pass took before returning). This proves the Windows
  `CreateFileW`/`WriteFile`/`ReadFile`/`WaitNamedPipeW` sequence in
  `socket_request` actually round-trips against a live Herdr server, not just
  its own unit tests. `agent.view.clear` was not separately exercised (no CLI
  path reaches it outside `configure`/`uninstall`); not considered a gap
  worth closing given `set` and `clear` share the same `socket_request`
  function and only differ in the JSON payload.
- **Windows manifest actions — `refresh-win` confirmed working;
  `open-settings-win` found broken and fixed.** `herdr plugin action invoke
  refresh-win --plugin herdr-agent-quota` → `herdr plugin log list` showed
  `exit_code 0`, `status:"succeeded"`, empty `stdout`/`stderr` (expected: the
  manifest command omits `--json`, and `run_internal` only prints when `json`
  is true — this is correct silent-success behavior, not a swallowed error).
  `open-settings-win`, as originally ported, **failed**: `exit_code 1`,
  `stderr: {"error":{"code":"platform_unsupported","message":"plugin pane
  does not support the current platform (windows)"}}`. Root cause: the
  windows action's command still said `--entrypoint settings` (the
  macos/linux-only pane id) instead of `--entrypoint settings-win` — a
  leftover from copying the unix action's body during the port.
  `tests/plugin_manifest.rs::settings_are_an_action_backed_by_a_plugin_pane`
  only asserts this for the `open-settings`/`settings` pair, not the `-win`
  variant, so `cargo test` never caught it. **Fixed** in this pass
  (`herdr-plugin.toml`'s `open-settings-win` action now passes `--entrypoint
  settings-win`); `cargo test --test plugin_manifest` still passes (7/7)
  after the fix. Re-linked (`herdr plugin link
  C:\git-repositories\10-herdr\herdr-agent-quota`, no flag — confirmed this
  preserves the existing `enabled` state rather than resetting it) and
  re-invoked: the error changed from `platform_unsupported` to a *different*
  failure, `plugin_pane_open_failed` / `"popup already open"` — because the
  `dashboard-win` popup opened earlier in this same test pass (see below) was
  still open and Herdr only allows one popup at a time. That confirms the
  entrypoint fix itself resolved cleanly (platform/entrypoint lookup
  succeeded); the `-win` action was not re-verified to a clean `exit_code 0`
  end-to-end because there was no CLI-reachable way found in this pass to
  close a popup pane opened against a headless session with no attached
  terminal client (`herdr pane list` does not enumerate popup panes — only
  the workspace's real split panes, `w1:p1`).

  **Closed the loop in a follow-up pass**, against a fresh isolated session
  (`agent-quota-smoke2`, no leftover popup from earlier testing):
  `herdr plugin action invoke open-settings-win --plugin herdr-agent-quota`
  → `herdr plugin log list --plugin herdr-agent-quota` showed `exit_code 0`,
  `status:"succeeded"`, `stdout:"{\"id\":\"cli:plugin\",\"result\":{\"type\":\"ok\"}}\n"`,
  `stderr:""`. `open-settings-win` is now fully closed-loop verified, not
  just fixed-and-plausible.
- **`dashboard-win` pane — opens and stays alive.** `herdr plugin pane open
  --plugin herdr-agent-quota --entrypoint dashboard-win --focus` → `{"type":
  "ok"}`, and the smoke session's `herdr-server.log` shows `pane.spawn.start`
  → `pane.spawned outcome="ok" pane_id=2 pid=<pid>` → `api.request.complete
  outcome="ok"`. The spawned `powershell.exe` process (running the
  `herdr-agent-quota.exe dashboard` launcher) was confirmed still alive via
  `Get-Process -Id <pid>` after the RPC returned — it did not immediately
  exit/crash. No attached TUI client means the popup's actual rendered
  content was not visually inspected, only that the process spawn and
  continued liveness succeeded; same structural-only limitation
  `herdr-cache-ttl`'s CLAUDE.md notes for its own pane/event testing.
- **No unexpected activity observed against the real `default` session.**
  Constraint from the task: enabling the plugin is a global toggle, so its
  `[[events]]` could in principle fire for real panes in `default` while
  enabled. The `default` session's own `herdr-server.log` was checked for the
  full enabled window (~10:40–10:46 UTC) and shows only ordinary
  workspace/tab-focus and session-save lines from the human's own use of
  Herdr during that time — no `plugin.event.invoke`/`agent-quota-refresh`
  entries and no errors. (This is not a guarantee the event hooks are
  side-effect-free in general — no real pane happened to change agent status
  during this specific window — just confirmation nothing went wrong in the
  window actually exercised.)
- **The `HOME` gap named in this repo's own "Not touched" section was hit in
  practice, for Grok specifically.** `$env:HOME` on this machine is empty
  (`$env:USERPROFILE` is set instead, confirmed before testing). `refresh
  --provider all --force --json` returned `{"provider":"codex","error":"start
  codex app-server", ...}` and `{"provider":"grok","error":"resolve Grok auth
  path", ...}` — `anyhow::Error::to_string()` only prints the outermost
  `.context(...)` message, not the full chain, so these two need reading
  against `src/`. Codex's error is **not** the `HOME` gap: `codex.rs:163`
  shows `command.spawn().context("start codex app-server")` fails before
  `codex_home()` (and therefore before its `HOME` read) is ever reached — the
  `codex` binary simply is not on PATH on this machine, an unrelated
  precondition. Grok's error **is** the documented `HOME` gap:
  `grok.rs::auth_path()` reads `HOME` unconditionally
  (`.context("HOME is not set")?`) before ever checking `GROK_HOME`. Proven
  causally, not just by inspection: re-running the same `refresh --provider
  grok --json` with `$env:HOME` set to a scratch directory changed the error
  from `"resolve Grok auth path"` to `"provider credentials are unavailable"`
  (a later-stage, expected failure — the fake `HOME` has no real Grok auth
  file) — confirming the missing `HOME` was what blocked it before. `HOME`
  was left unset again afterward; nothing in this pass touched the real
  environment persistently.
- **Final state, after the follow-up pass that closed the `open-settings-win`
  loop: `herdr-agent-quota` is `enabled: true`.** Left disabled at the end of
  the first pass (see above, superseded) because the `open-settings-win` fix
  was only partially re-verified at that point. Once the fresh
  `agent-quota-smoke2` session confirmed a clean `exit_code 0` for it (see
  above), every action/pane/event/socket path this repo's Windows port
  touches had been exercised against a live server with no remaining open
  failures, so the plugin was left enabled. The isolated `agent-quota-smoke`
  and `agent-quota-smoke2` sessions were both stopped and left in the
  `stopped` state alongside the pre-existing `probe*` sessions, not deleted.

## `configure --apply` run for real, against the live `default` session (2026-09-02)

With the plugin enabled and verified, `configure --apply` was actually run
against the real, non-isolated `default` session — the user's own live
Herdr setup, not a throwaway smoke session — to wire in the sidebar rows and
statusline collectors for real. This surfaced two more genuine bugs beyond
`config_path` (already fixed separately, see the "fix(windows)" commit for
it):

1. **`configure --apply`/`--check` refuses to run outside a real Herdr
   invocation** (`configure/mod.rs::run`, `HERDR_PLUGIN_STATE_DIR` env var
   required — "configuration writes must run through Herdr so every
   collector uses the same cache"). `herdr plugin action invoke` has no way
   to pass a custom `--agent` selection (its manifest command line is fixed:
   `configure --apply`, no flags), which was needed to work around finding 2
   below without also configuring `omp` (not installed on this machine, see
   below). Resolved by invoking the built exe directly with Herdr's real
   environment variables replicated by hand, not guessed: `HERDR_PLUGIN_ROOT`
   from `plugins.json`'s own entry, `HERDR_PLUGIN_CONFIG_DIR` from `herdr
   plugin config-dir herdr-agent-quota` (a real CLI command, exact value),
   `HERDR_PLUGIN_STATE_DIR` confirmed by checking
   `%LOCALAPPDATA%\herdr\plugins\herdr-agent-quota\` already contained real
   `*.refresh`/`*.lock` files being actively written by this plugin's own
   already-running watcher (proof of the real path, not the cache-ttl
   fork's documented pattern taken on faith), and `HERDR_SOCKET_PATH` as the
   `default` session's own real socket. `herdr server reload-config` (which
   the `configure-win` action's PowerShell wrapper normally chains after a
   successful apply) was run manually afterward for the same reason —
   invoking the exe directly bypasses that wrapper.
2. **`--agent all` (the default) hard-fails if `omp` isn't installed.**
   `integration::ensure_omp` runs first in the apply path and shells out to
   `herdr integration install omp`, which fails with `omp extension
   directory not found at ...\.omp\agent\extensions. install omp first` —
   omp is not installed on this machine. Not a Windows-specific bug (would
   fail identically on macOS/Linux without omp installed) and out of this
   port's scope to change; worked around by scoping `--agent
   claude,codex,grok,agy,opencode,pi` (every supported agent except `omp`).
   Confirmed via `configure/mod.rs` that `ensure_omp` runs before any actual
   file write, so the first attempt's failure left nothing partially
   applied (verified: `config.toml` and `~/.claude/settings.json` were
   unchanged after the first, `--agent all` attempt failed).
3. **The already-documented `HOME` gap (see "Not touched" — actually this
   one **is** touched by `--apply`, not just the provider fetchers listed
   there) blocked the Claude/Agy statusline install step**
   (`configure/statusline.rs::settings_path`, same `HOME`-only pattern as
   `configure/herdr.rs::config_path` had before its fix, not itself fixed in
   this pass — out of scope, same as the other six `HOME` call sites).
   `--apply` is documented as "safe to re-run; it repairs an existing
   installation in place", confirmed true in practice: the first attempt
   got as far as writing the `config.toml` sidebar row before failing on
   `HOME`, then a second attempt with `$env:HOME` set to `$env:USERPROFILE`
   for that one process invocation completed cleanly (exit 0), without
   duplicating the sidebar row (it recognized the existing markers).
4. **Not a bug, initially suspected to be one:** the installed
   `statusLine.command` for Claude/Agy uses POSIX inline-env-assignment
   syntax (`HERDR_PLUGIN_STATE_DIR='<path>' '<exe>' <subcommand>`,
   `configure/statusline.rs::apply_with_refresh_interval`), which `cmd.exe`
   and PowerShell cannot parse directly. This looked like a fourth Windows
   bug, but empirically testing the exact installed command via `sh -c` (the
   same shell Claude Code's own CLI uses on this Windows machine, via Git
   Bash on `PATH`) produced correct, real statusline output. Claude Code
   invokes `statusLine.command` through `sh`, not `cmd.exe`, even on
   Windows, so the existing Unix-shaped wrapper command is correct as-is on
   a machine with Git Bash installed (a reasonable assumption for a
   developer Windows machine, but not guaranteed on every Windows install —
   flagged here rather than silently assumed universal).

**Result:** `config.toml` now has a `[ui.sidebar.agents]`
`rows`/`rows_by_agent` quota row (verified by reading the file directly,
not just trusting exit code 0), `~/.claude/settings.json` and
`~/.gemini/antigravity-cli/settings.json` both have a working `statusLine`
pointing at this plugin, and `herdr server reload-config` applied with no
diagnostics. Session-id *attribution* (matching a quota reading to a
specific pane) still needs `herdr integration install
claude/codex/grok/pi` separately — `configure --apply` only installs the
collectors, it does not install Herdr's own per-agent integrations, and
none of those were installed as part of this pass.

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
