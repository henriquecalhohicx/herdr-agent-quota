use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// A statusLine command must never consume the refresh interval itself.
pub const STATUSLINE_COMMAND_BUDGET: Duration = Duration::from_secs(2);

/// Windows counterpart of the unix process group this module puts the
/// previous-statusLine shell into: a Job Object configured to kill every
/// process still assigned to it as soon as [`WindowsJob::terminate`] is
/// called (or, as a backstop, when the job's last handle closes — see
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` below).
///
/// Rust's `std::process::Command` has no Windows equivalent of unix
/// `pre_exec`, so unlike `setpgid(0, 0)` (which runs *inside* the child
/// before it execs anything else), the job can only be created and the child
/// assigned to it *after* `spawn()` returns. This leaves a small race window
/// between spawn and assignment during which a child that immediately
/// spawns a helper of its own could start that helper outside the job, so a
/// kill would miss it. Accepted as a known limitation rather than
/// over-engineering a suspended-start dance: this path only ever runs a
/// previous statusLine shell command, which in practice does not fork a
/// long-lived helper in that window.
#[cfg(windows)]
pub(crate) struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);

// SAFETY: a Win32 HANDLE is an opaque, thread-agnostic kernel object
// reference; every Win32 call this module makes on it (AssignProcessToJobObject,
// TerminateJobObject, CloseHandle) is documented as safe to call from any
// thread given a valid handle value.
#[cfg(windows)]
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
unsafe impl Sync for WindowsJob {}

#[cfg(windows)]
impl WindowsJob {
    /// Create a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` set, so
    /// that even if this process exits without calling `terminate()`
    /// explicitly (a panic, say), closing the job's last handle still kills
    /// anything still assigned to it — the same "nothing survives the
    /// parent" guarantee unix gets from the process group plus an explicit
    /// `killpg`.
    pub(crate) fn new() -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        // SAFETY: no name, no security attributes; the returned handle is
        // checked below and closed exactly once, in Drop.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = WindowsJob(handle);

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: handle is valid; info is a correctly sized, zero-initialized
        // struct with only the documented LimitFlags field set.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Assign a freshly spawned child to this job. See the race-window note
    /// on the struct itself for why "freshly" matters.
    pub(crate) fn assign(&self, child: &Child) -> std::io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let process = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        // SAFETY: `process` is the child's own process handle, valid for the
        // life of `child`; `self.0` is valid for the life of `self`.
        let ok = unsafe { AssignProcessToJobObject(self.0, process) };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// Kill every process still assigned to this job. The Windows analog of
    /// `killpg(pid, SIGKILL)`. Best-effort: a failure here (e.g. the job
    /// already has nothing left in it) is not worth surfacing, matching the
    /// unix path's ignored `killpg` return value.
    pub(crate) fn terminate(&self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: handle is valid for the life of `self`; exit code 1 is
        // arbitrary and unused by any caller here.
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        // SAFETY: handle came from CreateJobObjectW in `new` and is closed
        // exactly once, here.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Run a user-owned shell command with a hard wall-clock budget.
///
/// The child owns a process group so a timeout removes the shell and any
/// helper it started. stdout is drained concurrently, while the caller keeps
/// ownership of the child and can therefore kill and reap it without a pid
/// reuse race.
pub fn run_shell_with_deadline(
    command: &str,
    input: &[u8],
    budget: Duration,
) -> Result<CommandOutput> {
    let mut child = Command::new("sh");
    child
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    unsafe {
        child.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = child.spawn().context("run previous statusLine")?;
    // See the race-window note on `WindowsJob` for why this happens
    // immediately after spawn rather than inside a pre_exec-style hook (which
    // Windows has no equivalent of). A failure to create or assign the job is
    // not fatal to running the command at all — it just means a timeout below
    // falls back to killing only the immediate child, not its descendants.
    #[cfg(windows)]
    let job: Option<WindowsJob> = WindowsJob::new()
        .and_then(|job| job.assign(&child).map(|()| job))
        .ok();

    let stdin = child
        .stdin
        .take()
        .context("open previous statusLine stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("open previous statusLine stdout")?;
    let input = input.to_vec();
    let writer = thread::spawn(move || {
        let mut stdin = stdin;
        let _ = stdin.write_all(&input);
    });

    let child = Arc::new(Mutex::new(Some(child)));
    #[cfg(windows)]
    let job = Arc::new(job);
    let timed_out = Arc::new(AtomicBool::new(false));
    let (cancel, cancelled) = mpsc::channel();
    let watchdog_child = Arc::clone(&child);
    #[cfg(windows)]
    let watchdog_job = Arc::clone(&job);
    let watchdog_timed_out = Arc::clone(&timed_out);
    let watchdog = thread::spawn(move || {
        if cancelled.recv_timeout(budget).is_err() {
            watchdog_timed_out.store(true, Ordering::Release);
            #[cfg(unix)]
            let _ = terminate_and_reap(&watchdog_child);
            #[cfg(windows)]
            let _ = terminate_and_reap(&watchdog_child, watchdog_job.as_ref().as_ref());
        }
    });

    let mut stdout = stdout;
    let mut output = Vec::new();
    let read_result = stdout.read_to_end(&mut output);
    let _ = cancel.send(());
    #[cfg(unix)]
    let status = terminate_and_reap(&child)?;
    #[cfg(windows)]
    let status = terminate_and_reap(&child, job.as_ref().as_ref())?;
    let _ = watchdog.join();

    let _ = writer.join();
    read_result.context("read previous statusLine output")?;

    Ok(CommandOutput {
        stdout: output,
        exit_code: status.and_then(|status| status.code()),
        timed_out: timed_out.load(Ordering::Acquire),
    })
}

#[cfg(unix)]
fn terminate_and_reap(child: &Mutex<Option<Child>>) -> Result<Option<ExitStatus>> {
    let mut slot = child
        .lock()
        .map_err(|_| anyhow::anyhow!("lock previous statusLine child"))?;
    let Some(mut child) = slot.take() else {
        return Ok(None);
    };
    unsafe {
        // setpgid in the child setup makes this include shell descendants.
        let _ = libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    child.wait().map(Some).context("reap previous statusLine")
}

#[cfg(windows)]
fn terminate_and_reap(
    child: &Mutex<Option<Child>>,
    job: Option<&WindowsJob>,
) -> Result<Option<ExitStatus>> {
    let mut slot = child
        .lock()
        .map_err(|_| anyhow::anyhow!("lock previous statusLine child"))?;
    let Some(mut child) = slot.take() else {
        return Ok(None);
    };
    // The Job Object assignment in `run_shell_with_deadline` makes this
    // include any helper the shell command spawned, the same way `killpg`
    // does on unix.
    if let Some(job) = job {
        job.terminate();
    }
    let _ = child.kill();
    child.wait().map(Some).context("reap previous statusLine")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn captures_a_completed_command() {
        let result =
            run_shell_with_deadline("printf done", b"ignored", Duration::from_secs(1)).unwrap();
        assert_eq!(result.stdout, b"done");
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out);
    }

    #[test]
    fn kills_a_command_that_exceeds_its_budget() {
        let started = Instant::now();
        let result =
            run_shell_with_deadline("sleep 20 & wait", b"", Duration::from_millis(100)).unwrap();
        assert!(result.timed_out);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Direct test of the `WindowsJob` API against a real child process,
    /// independent of `run_shell_with_deadline`'s `sh`-dependent plumbing:
    /// create a job, assign a freshly spawned child to it, and terminate the
    /// job. The child's exit status must show it was killed mid-flight
    /// (`ping -n 5` takes ~4s; terminating immediately after assignment
    /// leaves no way for it to have exited on its own).
    ///
    /// This proves job creation, assignment, and termination all succeed
    /// end-to-end against a real Win32 process. It does NOT prove the
    /// job-object-specific guarantee this port actually cares about — that a
    /// *grandchild* the immediate child spawns is also killed, which is what
    /// distinguishes a Job Object from a plain `child.kill()`. Proving that
    /// needs a process tree deep enough to tell "the job killed it" apart
    /// from "killing the immediate child was enough on its own", which
    /// neither this test nor `kills_a_command_that_exceeds_its_budget` above
    /// attempts. Tracked as an honest gap in this repo's CLAUDE.md.
    #[cfg(windows)]
    #[test]
    fn windows_job_assigns_and_terminates_a_real_child() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping", "-n", "5", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let job = WindowsJob::new().expect("create job object");
        job.assign(&child).expect("assign child to job object");
        job.terminate();
        let status = child.wait().expect("reap terminated child");
        assert!(
            !status.success(),
            "child should have been killed before `ping -n 5` could finish on its own"
        );
    }
}
