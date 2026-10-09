//! End-to-end job control for `whykey listen --terminal` in a real pty.
//!
//! The binary runs as the foreground job of a fresh pty session, exactly as
//! under an interactive shell: a forked "shell" process becomes the session
//! leader, gives the terminal to whykey's own process group, and reports
//! `waitpid(WUNTRACED | WCONTINUED)` transitions back to the test. The shell
//! is required because Linux discards SIGTSTP for orphaned process groups, so
//! a bare session leader would never stop.
#![cfg(target_os = "linux")]

use std::collections::VecDeque;
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process;
use std::ptr;
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(15);
const POLL_MS: libc::c_int = 10;
const ESCAPE_RETRY: Duration = Duration::from_millis(50);
const KITTY_POP: &[u8] = b"\x1b[<u";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobEvent {
    Stopped(i32),
    Continued,
    Exited(i32),
    Signaled(i32),
}

/// One record on the status pipe: a tag byte and a little-endian `i32`.
const RECORD: usize = 5;

struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        // SAFETY: the descriptor is owned by this wrapper and closed once.
        unsafe { libc::close(self.0) };
    }
}

fn cstring(bytes: &[u8]) -> CString {
    CString::new(bytes).expect("no interior NUL")
}

fn null_terminated(strings: &[CString]) -> Vec<*const libc::c_char> {
    strings
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(ptr::null()))
        .collect()
}

fn set_cloexec(fd: libc::c_int) {
    // SAFETY: `fd` is an open descriptor owned by the caller.
    assert_ne!(
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
        -1
    );
}

/// Write one status record from the forked shell. Only async-signal-safe
/// calls are allowed there, so this neither allocates nor panics.
fn report(fd: libc::c_int, tag: u8, value: i32) {
    let mut record = [0_u8; RECORD];
    record[0] = tag;
    record[1..].copy_from_slice(&value.to_le_bytes());
    // SAFETY: `record` is a valid buffer of RECORD bytes.
    unsafe { libc::write(fd, record.as_ptr().cast(), RECORD) };
}

/// Body of the forked session leader. Never returns.
///
/// # Safety
/// Must run in a freshly forked child; it only uses async-signal-safe calls.
unsafe fn run_shell(
    slave: libc::c_int,
    status: libc::c_int,
    program: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
) -> ! {
    unsafe {
        if libc::setsid() == -1 || libc::ioctl(slave, libc::TIOCSCTTY, 0) == -1 {
            libc::_exit(120);
        }
        let job = libc::fork();
        if job == -1 {
            libc::_exit(121);
        }
        if job == 0 {
            // Become a foreground job, as a job-control shell does for the
            // child. SIGTTOU is blocked so tcsetpgrp from the new (still
            // background) group succeeds instead of stopping.
            libc::setpgid(0, 0);
            let mut ttou: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut ttou);
            libc::sigaddset(&mut ttou, libc::SIGTTOU);
            libc::sigprocmask(libc::SIG_BLOCK, &ttou, ptr::null_mut());
            if libc::tcsetpgrp(slave, libc::getpid()) == -1 {
                libc::_exit(122);
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, ptr::null_mut());
            // Foreground jobs start with default dispositions regardless of
            // what the test harness inherited.
            let mut default: libc::sigaction = std::mem::zeroed();
            default.sa_sigaction = libc::SIG_DFL;
            for signal in [
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGHUP,
                libc::SIGPIPE,
                libc::SIGCHLD,
            ] {
                libc::sigaction(signal, &default, ptr::null_mut());
            }
            for target in 0..=2 {
                libc::dup2(slave, target);
            }
            libc::execve(program, argv, envp);
            libc::_exit(127);
        }
        // Resolve the setpgid race the same way shells do: both sides call it.
        libc::setpgid(job, job);
        libc::close(slave);
        report(status, b'P', job);
        loop {
            let mut wait_status = 0;
            if libc::waitpid(job, &mut wait_status, libc::WUNTRACED | libc::WCONTINUED) == -1 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                libc::_exit(123);
            }
            if libc::WIFSTOPPED(wait_status) {
                report(status, b'S', libc::WSTOPSIG(wait_status));
            } else if libc::WIFCONTINUED(wait_status) {
                report(status, b'C', 0);
            } else if libc::WIFEXITED(wait_status) {
                report(status, b'E', libc::WEXITSTATUS(wait_status));
                libc::_exit(0);
            } else if libc::WIFSIGNALED(wait_status) {
                report(status, b'K', libc::WTERMSIG(wait_status));
                libc::_exit(0);
            }
        }
    }
}

/// `whykey listen --terminal` running as the foreground job of a pty.
struct PtyJob {
    master: Fd,
    /// Held open so the pty (and its termios) outlives the job.
    _slave: Fd,
    /// Termios captured right after `openpty`, before any process could
    /// touch the pty: the state every restoration is compared against.
    baseline: libc::termios,
    status: Fd,
    shell: libc::pid_t,
    job: libc::pid_t,
    output: Vec<u8>,
    status_bytes: Vec<u8>,
    events: VecDeque<JobEvent>,
    finished: bool,
    home: PathBuf,
}

impl PtyJob {
    fn spawn(label: &str) -> Self {
        let home =
            std::env::temp_dir().join(format!("whykey-listen-pty-{}-{label}", process::id()));
        let _ = fs::remove_dir_all(&home);
        let empty_path = home.join("bin");
        fs::create_dir_all(&empty_path).expect("create temp HOME");

        // Everything the forked processes touch is prepared up front: after
        // fork only async-signal-safe calls run.
        let program = cstring(env!("CARGO_BIN_EXE_whykey").as_bytes());
        let argv = [
            cstring(b"whykey"),
            cstring(b"listen"),
            cstring(b"--terminal"),
        ];
        // A fully explicit environment: no compositor, multiplexer, or host
        // terminal identity, and an empty PATH so no helper tool can run.
        let mut env = vec![
            [b"HOME=".as_slice(), home.as_os_str().as_bytes()].concat(),
            [
                b"XDG_CONFIG_HOME=".as_slice(),
                home.join("config").as_os_str().as_bytes(),
            ]
            .concat(),
            [b"PATH=".as_slice(), empty_path.as_os_str().as_bytes()].concat(),
        ];
        env.extend([
            b"TERM=xterm-256color".to_vec(),
            b"LANG=C.UTF-8".to_vec(),
            b"NO_COLOR=1".to_vec(),
        ]);
        let env: Vec<CString> = env.iter().map(|entry| cstring(entry)).collect();
        let argv_ptrs = null_terminated(&argv);
        let env_ptrs = null_terminated(&env);

        let (mut master, mut slave) = (0, 0);
        // SAFETY: both out-pointers are valid; name/termios/winsize may be null.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
            )
        };
        assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
        let (master, slave) = (Fd(master), Fd(slave));
        set_cloexec(master.0);
        set_cloexec(slave.0);
        let baseline = read_termios(master.0);
        let mut pipe = [0; 2];
        // SAFETY: `pipe` has room for two descriptors.
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        let (status, status_write) = (Fd(pipe[0]), Fd(pipe[1]));

        // SAFETY: the child only runs `run_shell`, which is async-signal-safe.
        let shell = unsafe { libc::fork() };
        assert_ne!(shell, -1, "fork: {}", std::io::Error::last_os_error());
        if shell == 0 {
            // SAFETY: freshly forked child; every pointer was built above.
            unsafe {
                run_shell(
                    slave.0,
                    status_write.0,
                    program.as_ptr(),
                    argv_ptrs.as_ptr(),
                    env_ptrs.as_ptr(),
                )
            }
        }
        drop(status_write);

        let mut job = Self {
            master,
            _slave: slave,
            baseline,
            status,
            shell,
            job: 0,
            output: Vec::new(),
            status_bytes: Vec::new(),
            events: VecDeque::new(),
            finished: false,
            home,
        };
        job.wait_until("job pid from the shell", |job| job.job != 0);
        job
    }

    /// Read whatever the job wrote and any shell status records, waiting at
    /// most one short poll interval.
    fn pump(&mut self) {
        let mut fds = [
            libc::pollfd {
                fd: self.master.0,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.status.0,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let count = if self.finished { 1 } else { 2 };
        // SAFETY: `fds` holds `count` valid pollfd entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, POLL_MS) };
        if ready <= 0 {
            return;
        }
        let mut buffer = [0_u8; 4096];
        if fds[0].revents & libc::POLLIN != 0 {
            // SAFETY: `buffer` is valid writable storage of its length.
            let read =
                unsafe { libc::read(self.master.0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read > 0 {
                self.output.extend_from_slice(&buffer[..read as usize]);
            }
        }
        if !self.finished && fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
            // SAFETY: as above.
            let read =
                unsafe { libc::read(self.status.0, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read <= 0 {
                self.finished = true;
                return;
            }
            self.status_bytes
                .extend_from_slice(&buffer[..read as usize]);
            while self.status_bytes.len() >= RECORD {
                let record: Vec<u8> = self.status_bytes.drain(..RECORD).collect();
                let value = i32::from_le_bytes(record[1..].try_into().unwrap());
                match record[0] {
                    b'P' => self.job = value,
                    b'S' => self.events.push_back(JobEvent::Stopped(value)),
                    b'C' => self.events.push_back(JobEvent::Continued),
                    b'E' => self.events.push_back(JobEvent::Exited(value)),
                    b'K' => self.events.push_back(JobEvent::Signaled(value)),
                    tag => panic!("unknown status record {tag}"),
                }
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        while !done(self) {
            if Instant::now() >= deadline {
                panic!(
                    "timed out waiting for {what}; events {:?}; output {:?}",
                    self.events,
                    String::from_utf8_lossy(&self.output)
                );
            }
            self.pump();
        }
    }

    fn next_event(&mut self, what: &str) -> JobEvent {
        self.wait_until(what, |job| !job.events.is_empty());
        self.events.pop_front().unwrap()
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: `job` is the live whykey process created by our shell.
        assert_eq!(unsafe { libc::kill(self.job, signal) }, 0);
    }

    fn termios(&self) -> libc::termios {
        // On Linux a pty master reports the shared termios of the pair.
        read_termios(self.master.0)
    }

    fn raw_mode(&self) -> bool {
        self.termios().c_lflag & libc::ICANON == 0
    }

    fn pushes(&self) -> Vec<usize> {
        kitty_pushes(&self.output)
    }

    fn pops(&self) -> Vec<usize> {
        occurrences(&self.output, KITTY_POP)
    }

    /// Wait until capture is armed for the `nth` time (1-based): raw mode
    /// is on and the kitty push sequence has been written.
    fn wait_armed(&mut self, nth: usize) {
        self.wait_until(&format!("capture armed #{nth}"), |job| {
            job.raw_mode() && job.pushes().len() >= nth
        });
    }

    /// Press Escape until the job exits. Escape bytes that reach the pty
    /// before capture flushes pending input are discarded by design, so
    /// the key is re-sent at a short interval instead of guessing timing.
    fn cancel_with_escape(&mut self) -> JobEvent {
        let deadline = Instant::now() + DEADLINE;
        let mut last_press: Option<Instant> = None;
        loop {
            if let Some(event) = self.events.pop_front() {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "Escape never cancelled capture; output {:?}",
                String::from_utf8_lossy(&self.output)
            );
            if last_press.is_none_or(|at| at.elapsed() >= ESCAPE_RETRY) {
                // SAFETY: writes one byte from a valid buffer.
                unsafe { libc::write(self.master.0, b"\x1b".as_ptr().cast(), 1) };
                last_press = Some(Instant::now());
            }
            self.pump();
        }
    }
}

impl Drop for PtyJob {
    fn drop(&mut self) {
        // SAFETY: both pids belong to processes this test forked; killing an
        // already-reaped pid is avoided by checking `finished` for the shell.
        unsafe {
            if !self.finished {
                if self.job > 0 {
                    libc::kill(self.job, libc::SIGKILL);
                }
                libc::kill(self.shell, libc::SIGKILL);
            }
            libc::waitpid(self.shell, ptr::null_mut(), 0);
        }
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn read_termios(fd: libc::c_int) -> libc::termios {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(
        // SAFETY: `termios` is a valid out-pointer for tcgetattr.
        unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) },
        0,
        "tcgetattr: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: tcgetattr initialized it.
    unsafe { termios.assume_init() }
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(index, _)| index)
        .collect()
}

/// Positions of complete kitty push sequences `ESC [ > <digits> u`.
fn kitty_pushes(output: &[u8]) -> Vec<usize> {
    occurrences(output, b"\x1b[>")
        .into_iter()
        .filter(|&start| {
            let rest = &output[start + 3..];
            let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
            digits > 0 && rest.get(digits) == Some(&b'u')
        })
        .collect()
}

fn same_termios(left: &libc::termios, right: &libc::termios) -> bool {
    left.c_iflag == right.c_iflag
        && left.c_oflag == right.c_oflag
        && left.c_cflag == right.c_cflag
        && left.c_lflag == right.c_lflag
        && left.c_cc == right.c_cc
}

fn assert_restored(job: &PtyJob, original: &libc::termios, when: &str) {
    let current = job.termios();
    assert!(
        same_termios(&current, original),
        "termios not restored {when}: lflag {:#x} vs {:#x}, iflag {:#x} vs {:#x}",
        current.c_lflag,
        original.c_lflag,
        current.c_iflag,
        original.c_iflag
    );
}

/// Every push so far is followed by a pop, in order.
fn assert_balanced(job: &PtyJob, pushes: usize) {
    let push_at = job.pushes();
    let pop_at = job.pops();
    assert_eq!(push_at.len(), pushes, "kitty pushes");
    assert_eq!(pop_at.len(), pushes, "kitty pops");
    for (push, pop) in push_at.iter().zip(&pop_at) {
        assert!(pop > push, "kitty pop at {pop} precedes its push at {push}");
    }
    for (pop, next_push) in pop_at.iter().zip(push_at.iter().skip(1)) {
        assert!(
            next_push > pop,
            "push at {next_push} precedes earlier pop at {pop}"
        );
    }
}

#[test]
fn terminal_listen_suspends_restores_and_resumes_under_job_control() {
    let mut job = PtyJob::spawn("tstp");
    let original = job.baseline;
    assert_ne!(original.c_lflag & libc::ICANON, 0, "fresh pty is canonical");

    job.wait_armed(1);

    job.signal(libc::SIGTSTP);
    assert_eq!(
        job.next_event("stop after SIGTSTP"),
        JobEvent::Stopped(libc::SIGTSTP)
    );
    // The pop was written before stopping; it may still be in the pty buffer.
    job.wait_until("kitty pop before stop", |job| !job.pops().is_empty());
    assert_restored(&job, &original, "while stopped");
    assert_balanced(&job, 1);

    job.signal(libc::SIGCONT);
    assert_eq!(
        job.next_event("continue after SIGCONT"),
        JobEvent::Continued
    );
    job.wait_armed(2);

    assert_eq!(job.cancel_with_escape(), JobEvent::Exited(0));
    job.wait_until("kitty pop on exit", |job| job.pops().len() >= 2);
    assert_restored(&job, &original, "after Escape");
    assert_balanced(&job, 2);
}

#[test]
fn terminal_listen_restores_the_terminal_on_sigterm() {
    let mut job = PtyJob::spawn("term");
    let original = job.baseline;

    job.wait_armed(1);

    job.signal(libc::SIGTERM);
    let event = job.next_event("exit after SIGTERM");
    assert!(
        matches!(event, JobEvent::Exited(code) if code != 0),
        "SIGTERM must end capture through its own cleanup with a failure status, got {event:?}"
    );
    job.wait_until("kitty pop on SIGTERM", |job| !job.pops().is_empty());
    assert_restored(&job, &original, "after SIGTERM");
    assert_balanced(&job, 1);
}
