//! Bounded execution for optional integration commands.
//!
//! Diagnostics query programs that belong to the user's desktop session. A
//! broken IPC endpoint must not be able to hang a whole report, and a command
//! should not be able to fill memory by writing unbounded output. This module
//! keeps those rules in one place while returning the standard `Output` type
//! to the adapters.

use std::fmt;
use std::io::{self, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_MAX_OUTPUT: usize = 1024 * 1024;
pub const DEFAULT_DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(10);

const POLL_INTERVAL: Duration = Duration::from_millis(5);

thread_local! {
    static ACTIVE_DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

struct DeadlineGuard {
    previous: Option<Instant>,
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        ACTIVE_DEADLINE.with(|deadline| deadline.set(self.previous));
    }
}

#[derive(Debug)]
pub enum CommandError {
    Spawn {
        program: String,
        source: io::Error,
    },
    Wait {
        program: String,
        source: io::Error,
    },
    Kill {
        program: String,
        source: io::Error,
    },
    Read {
        program: String,
        stream: &'static str,
        source: io::Error,
    },
    Write {
        program: String,
        source: io::Error,
    },
    ReaderPanicked {
        program: String,
        stream: &'static str,
    },
    TimedOut {
        program: String,
        timeout: Duration,
    },
    OutputLimit {
        program: String,
        stream: &'static str,
        limit: usize,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, source } => {
                write!(formatter, "failed to run {program}: {source}")
            }
            Self::Wait { program, source } => {
                write!(formatter, "failed waiting for {program}: {source}")
            }
            Self::Kill { program, source } => {
                write!(formatter, "failed stopping {program}: {source}")
            }
            Self::Read {
                program,
                stream,
                source,
            } => {
                write!(
                    formatter,
                    "failed reading {stream} from {program}: {source}"
                )
            }
            Self::Write { program, source } => {
                write!(formatter, "failed writing request to {program}: {source}")
            }
            Self::ReaderPanicked { program, stream } => {
                write!(formatter, "reader for {stream} from {program} panicked")
            }
            Self::TimedOut { program, timeout } => {
                write!(
                    formatter,
                    "{program} timed out after {} ms",
                    timeout.as_millis()
                )
            }
            Self::OutputLimit {
                program,
                stream,
                limit,
            } => write!(
                formatter,
                "{program} exceeded the {limit}-byte {stream} output limit"
            ),
        }
    }
}

impl std::error::Error for CommandError {}

#[derive(Debug)]
struct ReadResult {
    bytes: Vec<u8>,
    overflowed: bool,
    error: Option<io::Error>,
}

/// Run a command with the standard diagnostic timeout and output cap.
pub fn output(command: &mut Command) -> Result<Output, CommandError> {
    let timeout = match deadline_remaining() {
        Some(remaining) => configured_timeout().min(remaining),
        None => configured_timeout(),
    };
    output_with_limits(command, timeout, configured_max_output())
}

/// Run a diagnostic operation with one shared wall-clock budget.
///
/// Nested calls cannot extend an outer deadline. The guard is thread-local so
/// independent listeners or tests do not affect one another.
pub fn with_deadline<T>(duration: Duration, operation: impl FnOnce() -> T) -> T {
    let now = Instant::now();
    let requested = now
        .checked_add(duration)
        .unwrap_or_else(|| now + Duration::from_secs(365 * 24 * 60 * 60));
    let previous = ACTIVE_DEADLINE.with(|deadline| {
        let previous = deadline.get();
        let effective = previous.map_or(requested, |outer| outer.min(requested));
        deadline.set(Some(effective));
        previous
    });
    let _guard = DeadlineGuard { previous };
    operation()
}

pub fn configured_diagnostic_timeout() -> Duration {
    std::env::var("WHYKEY_DIAGNOSTIC_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_DIAGNOSTIC_TIMEOUT)
}

pub fn deadline_exceeded() -> bool {
    ACTIVE_DEADLINE.with(|deadline| deadline.get().is_some_and(|value| Instant::now() >= value))
}

pub fn deadline_remaining() -> Option<Duration> {
    ACTIVE_DEADLINE.with(|deadline| {
        deadline
            .get()
            .map(|value| value.saturating_duration_since(Instant::now()))
    })
}

fn configured_timeout() -> Duration {
    std::env::var("WHYKEY_COMMAND_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT)
}

fn configured_max_output() -> usize {
    std::env::var("WHYKEY_COMMAND_MAX_OUTPUT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_OUTPUT)
}

/// Run a command with explicit limits. Both stdout and stderr are drained in
/// parallel so a noisy child cannot deadlock while the parent waits for it.
pub fn output_with_limits(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> Result<Output, CommandError> {
    output_with_limits_and_input(command, None, timeout, max_output)
}

/// Run a command with a bounded request on stdin.
///
/// This is used by the opt-in extension protocol. The request is written
/// before waiting for the child and stdin is then closed, so an extension
/// cannot inherit an interactive input stream or wait for unrelated input.
pub fn output_with_input(command: &mut Command, input: &[u8]) -> Result<Output, CommandError> {
    let timeout = match deadline_remaining() {
        Some(remaining) => configured_timeout().min(remaining),
        None => configured_timeout(),
    };
    output_with_limits_and_input(command, Some(input), timeout, configured_max_output())
}

fn output_with_limits_and_input(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: Duration,
    max_output: usize,
) -> Result<Output, CommandError> {
    let program = command.get_program().to_string_lossy().into_owned();
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    isolate_process_group(command);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CommandError::Spawn {
            program: program.clone(),
            source,
        })?;
    let stdout = child.stdout.take().ok_or_else(|| CommandError::Read {
        program: program.clone(),
        stream: "stdout",
        source: io::Error::other("stdout pipe was not created"),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| CommandError::Read {
        program: program.clone(),
        stream: "stderr",
        source: io::Error::other("stderr pipe was not created"),
    })?;

    if let Some(input) = input {
        let write_result = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("stdin pipe was not created"))
            .and_then(|mut stdin| {
                stdin.write_all(input)?;
                Ok(())
            });
        if let Err(source) = write_result {
            let _ = stop_child(&mut child, &program);
            let _ = child.wait();
            return Err(CommandError::Write { program, source });
        }
    }

    let stdout_overflowed = Arc::new(AtomicBool::new(false));
    let stderr_overflowed = Arc::new(AtomicBool::new(false));
    let stdout_flag = Arc::clone(&stdout_overflowed);
    let stderr_flag = Arc::clone(&stderr_overflowed);
    let stdout_thread = thread::spawn(move || read_limited(stdout, max_output, stdout_flag));
    let stderr_thread = thread::spawn(move || read_limited(stderr, max_output, stderr_flag));

    let started = Instant::now();
    let mut timed_out = false;
    loop {
        let output_limit_hit =
            stdout_overflowed.load(Ordering::Acquire) || stderr_overflowed.load(Ordering::Acquire);
        if output_limit_hit {
            stop_child(&mut child, &program)?;
            break;
        }
        match child.try_wait().map_err(|source| CommandError::Wait {
            program: program.clone(),
            source,
        })? {
            Some(_) => {
                // A query may have left a helper holding stdout/stderr open.
                // The helper belongs to this private group, so close it
                // before joining the reader threads.
                kill_process_group(child.id());
                break;
            }
            None if started.elapsed() >= timeout => {
                timed_out = true;
                stop_child(&mut child, &program)?;
                break;
            }
            None => thread::sleep(POLL_INTERVAL),
        }
    }

    let status = child.wait().map_err(|source| CommandError::Wait {
        program: program.clone(),
        source,
    })?;
    let stdout = join_reader(stdout_thread, &program, "stdout")?;
    let stderr = join_reader(stderr_thread, &program, "stderr")?;
    if timed_out {
        return Err(CommandError::TimedOut { program, timeout });
    }
    if stdout.overflowed {
        return Err(CommandError::OutputLimit {
            program,
            stream: "stdout",
            limit: max_output,
        });
    }
    if stderr.overflowed {
        return Err(CommandError::OutputLimit {
            program,
            stream: "stderr",
            limit: max_output,
        });
    }
    if let Some(source) = stdout.error {
        return Err(CommandError::Read {
            program,
            stream: "stdout",
            source,
        });
    }
    if let Some(source) = stderr.error {
        return Err(CommandError::Read {
            program,
            stream: "stderr",
            source,
        });
    }
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn stop_child(child: &mut std::process::Child, program: &str) -> Result<(), CommandError> {
    kill_process_group(child.id());
    match child.kill() {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(source) => Err(CommandError::Kill {
            program: program.to_owned(),
            source,
        }),
    }
}

fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    // SAFETY: pre_exec runs in the child between fork and exec. setpgid only
    // changes the child process group and does not access Rust-managed state.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn kill_process_group(pid: u32) {
    #[cfg(unix)]
    {
        let process_group = -(pid as libc::pid_t);
        // The child was placed in a private group by isolate_process_group.
        // ESRCH simply means the group already exited.
        let _ = unsafe { libc::kill(process_group, libc::SIGKILL) };
    }
}

fn join_reader(
    handle: thread::JoinHandle<ReadResult>,
    program: &str,
    stream: &'static str,
) -> Result<ReadResult, CommandError> {
    handle.join().map_err(|_| CommandError::ReaderPanicked {
        program: program.to_owned(),
        stream,
    })
}

fn read_limited<R: Read>(
    mut reader: R,
    max_output: usize,
    overflow_flag: Arc<AtomicBool>,
) -> ReadResult {
    let mut bytes = Vec::with_capacity(max_output.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut overflowed = false;
    let mut error = None;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let previous_len = bytes.len();
                if previous_len < max_output {
                    let retained = count.min(max_output - previous_len);
                    bytes.extend_from_slice(&buffer[..retained]);
                }
                if count > max_output.saturating_sub(previous_len) {
                    overflowed = true;
                    overflow_flag.store(true, Ordering::Release);
                }
            }
            Err(source) => {
                error = Some(source);
                break;
            }
        }
    }
    ReadResult {
        bytes,
        overflowed,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn enforces_a_timeout() {
        let started = Instant::now();
        let result = output_with_limits(
            Command::new("sh").arg("-c").arg("sleep 1"),
            Duration::from_millis(40),
            1024,
        );
        assert!(matches!(result, Err(CommandError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn caps_noisy_output_without_hanging() {
        let result = output_with_limits(
            Command::new("sh").arg("-c").arg("printf '%02048d' 0"),
            Duration::from_secs(1),
            64,
        );
        assert!(matches!(
            result,
            Err(CommandError::OutputLimit {
                stream: "stdout",
                ..
            })
        ));
    }

    #[test]
    fn preserves_successful_output_and_status() {
        let output = output_with_limits(
            Command::new("sh").arg("-c").arg("printf ok"),
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"ok");
    }

    #[test]
    fn writes_a_bounded_request_and_closes_stdin() {
        let output = output_with_input(
            Command::new("sh")
                .arg("-c")
                .arg("read value; printf '%s' \"$value\""),
            b"whykey\n",
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"whykey");
    }

    #[test]
    fn accepts_output_exactly_at_the_limit() {
        let output = output_with_limits(
            Command::new("sh").arg("-c").arg("printf '%064d' 0"),
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert_eq!(output.stdout.len(), 64);
    }

    #[test]
    fn closes_inherited_pipes_when_a_child_survives_the_query() {
        let started = Instant::now();
        let output = output_with_limits(
            Command::new("sh").arg("-c").arg("sleep 1 & exit 0"),
            Duration::from_secs(1),
            64,
        )
        .unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn clamps_a_query_to_the_active_diagnostic_deadline() {
        let started = Instant::now();
        let result = with_deadline(Duration::from_millis(40), || {
            output(Command::new("sh").arg("-c").arg("sleep 1"))
        });

        assert!(matches!(result, Err(CommandError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn restores_the_previous_deadline_after_nested_work() {
        assert!(!deadline_exceeded());
        with_deadline(Duration::from_secs(1), || {
            assert!(!deadline_exceeded());
            with_deadline(Duration::ZERO, || assert!(deadline_exceeded()));
            assert!(!deadline_exceeded());
        });
        assert!(!deadline_exceeded());
    }
}
