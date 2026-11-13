//! The terminal settings of the process an inspection targets (`inspect
//! --pid`, `--focused`).
//!
//! The TTY driver judges a byte against termios, and termios belongs to a
//! terminal device, not to whykey. When the target is another program (vim,
//! less, a shell in another tmux pane) its device may be in raw mode with
//! ISIG, IXON and ICANON cleared, while whykey's own terminal is cooked. So
//! the device is found from the target's `/proc/<pid>/stat` (`tty_nr`, else
//! its standard input) and read, never changed. When it cannot be read the
//! caller must say so; it must not fall back to whykey's own terminal.

use crate::focus::{ProcStat, proc_stat};

/// Termios of the target's terminal plus where it came from, as evidence.
pub struct TargetTermios {
    pub termios: libc::termios,
    pub details: Vec<String>,
}

/// Which handle named the terminal device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    ControllingTerminal,
    StandardInput,
}

/// Read the termios of the terminal of process `pid`. The error text says
/// why the settings are unknown.
pub fn read(pid: u32) -> Result<TargetTermios, String> {
    let stat = proc_stat(pid);
    let standard_input = crate::recording::read_link(format!("/proc/{pid}/fd/0"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let (path, via) = select_device(pid, stat.as_ref(), standard_input.as_deref())?;
    let termios = crate::recording::tty_termios_of(&path)
        .map_err(|error| format!("{error} (the terminal of process {pid})"))?;
    // `tcgetpgrp` only works on the caller's own controlling terminal, so the
    // foreground group is the target's `tpgid`.
    let foreground_command = stat
        .as_ref()
        .filter(|_| via == Via::ControllingTerminal)
        .filter(|stat| stat.foreground_group > 0)
        .and_then(|stat| {
            crate::recording::read_to_string(format!("/proc/{}/comm", stat.foreground_group)).ok()
        });
    Ok(TargetTermios {
        termios,
        details: describe(
            pid,
            stat.as_ref(),
            &path,
            via,
            foreground_command.as_deref().map(str::trim),
        ),
    })
}

/// The device path for the target: its controlling terminal, or the terminal
/// on its standard input when it has none.
fn select_device(
    pid: u32,
    stat: Option<&ProcStat>,
    standard_input: Option<&str>,
) -> Result<(String, Via), String> {
    let Some(stat) = stat else {
        return Err(format!("process {pid} is not readable under /proc"));
    };
    if stat.tty != 0 {
        return match device_path(stat.tty) {
            Some(path) => Ok((path, Via::ControllingTerminal)),
            None => Err(format!(
                "the controlling terminal of process {pid} (device {}:{}) has no known /dev path",
                device_major(stat.tty),
                device_minor(stat.tty)
            )),
        };
    }
    match standard_input.and_then(terminal_path) {
        Some(path) => Ok((path, Via::StandardInput)),
        None => Err(format!(
            "process {pid} has no controlling terminal and its standard input is not a terminal"
        )),
    }
}

/// Evidence lines naming the process and device the settings came from.
fn describe(
    pid: u32,
    stat: Option<&ProcStat>,
    path: &str,
    via: Via,
    foreground_command: Option<&str>,
) -> Vec<String> {
    let mut details = vec![match via {
        Via::ControllingTerminal => {
            format!("termios read from {path}, the controlling terminal of pid {pid}")
        }
        Via::StandardInput => format!("termios read from {path}, standard input of pid {pid}"),
    }];
    let Some(stat) = stat.filter(|_| via == Via::ControllingTerminal) else {
        return details;
    };
    if stat.foreground_group > 0 {
        details.push(
            match foreground_command.filter(|command| !command.is_empty()) {
                Some(command) => format!(
                    "foreground process group {} ({command}) reads the terminal",
                    stat.foreground_group
                ),
                None => format!(
                    "foreground process group {} reads the terminal",
                    stat.foreground_group
                ),
            },
        );
        if stat.group != stat.foreground_group {
            details.push(format!(
                "pid {pid} is in process group {}, not the foreground group; the settings are \
                 the foreground program's, which is where the key goes",
                stat.group
            ));
        }
    }
    details
}

fn terminal_path(path: &str) -> Option<String> {
    let path = path.strip_suffix(" (deleted)").unwrap_or(path);
    (path.starts_with("/dev/pts/") || path.starts_with("/dev/tty")).then(|| path.to_string())
}

fn device_major(device: u64) -> u64 {
    (device >> 8) & 0xfff
}

fn device_minor(device: u64) -> u64 {
    (device & 0xff) | ((device >> 12) & 0xfff00)
}

/// The `/dev` path of a terminal device number as `/proc/<pid>/stat` reports
/// it: pseudo-terminal slaves (majors 136 to 143), virtual consoles and
/// serial lines (major 4).
fn device_path(device: u64) -> Option<String> {
    let (major, minor) = (device_major(device), device_minor(device));
    match major {
        136..=143 => Some(format!("/dev/pts/{}", (major - 136) * 256 + minor)),
        4 if minor < 64 => Some(format!("/dev/tty{minor}")),
        4 => Some(format!("/dev/ttyS{}", minor - 64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(group: i64, tty: u64, foreground_group: i64) -> ProcStat {
        ProcStat {
            pid: 10,
            group,
            tty,
            foreground_group,
        }
    }

    #[test]
    fn device_numbers_map_to_dev_paths() {
        // /dev/pts/3 is 136:3, which /proc/<pid>/stat writes as 34819.
        assert_eq!(device_path(34819).as_deref(), Some("/dev/pts/3"));
        // Minor numbers above 255 spill into the high bits: 136:300 is pts/300.
        let pts300 = (300 & 0xff) | (136 << 8) | ((300u64 & !0xff) << 12);
        assert_eq!(device_path(pts300).as_deref(), Some("/dev/pts/300"));
        assert_eq!(device_path((4 << 8) | 2).as_deref(), Some("/dev/tty2"));
        assert_eq!(device_path((4 << 8) | 65).as_deref(), Some("/dev/ttyS1"));
        assert_eq!(device_path(5 << 8), None);
    }

    #[test]
    fn the_controlling_terminal_names_the_device() {
        let target = stat(10, 34819, 10);
        assert_eq!(
            select_device(10, Some(&target), Some("/dev/null")),
            Ok(("/dev/pts/3".into(), Via::ControllingTerminal))
        );
    }

    #[test]
    fn standard_input_is_used_only_without_a_controlling_terminal() {
        let target = stat(10, 0, -1);
        assert_eq!(
            select_device(10, Some(&target), Some("/dev/pts/7")),
            Ok(("/dev/pts/7".into(), Via::StandardInput))
        );
        let error = select_device(10, Some(&target), Some("pipe:[1234]")).unwrap_err();
        assert!(error.contains("no controlling terminal"), "{error}");
        let error = select_device(10, Some(&target), None).unwrap_err();
        assert!(error.contains("not a terminal"), "{error}");
    }

    #[test]
    fn an_unreadable_process_or_unknown_device_is_an_error() {
        assert!(
            select_device(10, None, Some("/dev/pts/7"))
                .unwrap_err()
                .contains("not readable under /proc")
        );
        let odd = stat(10, (240 << 8) | 1, 10);
        assert!(
            select_device(10, Some(&odd), None)
                .unwrap_err()
                .contains("device 240:1")
        );
    }

    #[test]
    fn evidence_names_the_process_device_and_foreground_program() {
        let shell_running_vim = stat(10, 34819, 20);
        let details = describe(
            10,
            Some(&shell_running_vim),
            "/dev/pts/3",
            Via::ControllingTerminal,
            Some("vim"),
        );
        assert_eq!(
            details[0],
            "termios read from /dev/pts/3, the controlling terminal of pid 10"
        );
        assert_eq!(
            details[1],
            "foreground process group 20 (vim) reads the terminal"
        );
        assert!(
            details[2].contains("not the foreground group"),
            "{details:?}"
        );

        let in_foreground = stat(20, 34819, 20);
        let details = describe(
            20,
            Some(&in_foreground),
            "/dev/pts/3",
            Via::ControllingTerminal,
            None,
        );
        assert_eq!(details.len(), 2, "{details:?}");

        let details = describe(10, None, "/dev/pts/7", Via::StandardInput, None);
        assert_eq!(
            details,
            ["termios read from /dev/pts/7, standard input of pid 10"]
        );
    }

    #[test]
    fn only_terminal_devices_are_read_from_standard_input() {
        assert_eq!(terminal_path("/dev/pts/4").as_deref(), Some("/dev/pts/4"));
        assert_eq!(terminal_path("/dev/tty2").as_deref(), Some("/dev/tty2"));
        assert_eq!(terminal_path("/dev/null"), None);
        assert_eq!(terminal_path("/tmp/x"), None);
        assert_eq!(terminal_path("socket:[1]"), None);
    }

    /// A pseudo-terminal whose slave is the controlling terminal of a child
    /// process, as a terminal emulator's pty is the terminal of its shell.
    struct Pty {
        master: libc::c_int,
        slave: libc::c_int,
        path: String,
    }

    impl Pty {
        fn open() -> Pty {
            unsafe {
                let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
                assert!(master >= 0, "posix_openpt failed");
                assert_eq!(libc::grantpt(master), 0);
                assert_eq!(libc::unlockpt(master), 0);
                let mut name = [0 as libc::c_char; 128];
                assert_eq!(libc::ptsname_r(master, name.as_mut_ptr(), name.len()), 0);
                let path = std::ffi::CStr::from_ptr(name.as_ptr())
                    .to_string_lossy()
                    .into_owned();
                let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
                assert!(slave >= 0, "cannot open {path}");
                Pty {
                    master,
                    slave,
                    path,
                }
            }
        }

        /// Set the flags the way a program does when it takes over the tty.
        fn set_flags(&self, lflag: libc::tcflag_t, iflag: libc::tcflag_t) {
            unsafe {
                let mut termios = std::mem::zeroed::<libc::termios>();
                assert_eq!(libc::tcgetattr(self.slave, &mut termios), 0);
                termios.c_lflag = lflag;
                termios.c_iflag = iflag;
                assert_eq!(libc::tcsetattr(self.slave, libc::TCSANOW, &termios), 0);
            }
        }
    }

    impl Drop for Pty {
        fn drop(&mut self) {
            unsafe {
                libc::close(self.slave);
                libc::close(self.master);
            }
        }
    }

    #[test]
    fn reads_a_pty_without_changing_it() {
        let pty = Pty::open();
        pty.set_flags(libc::ISIG | libc::ICANON, libc::IXON);
        let cooked = crate::recording::tty_termios_of(&pty.path).expect("readable pty");
        assert_ne!(cooked.c_lflag & libc::ISIG, 0);
        assert_ne!(cooked.c_iflag & libc::IXON, 0);

        pty.set_flags(0, 0);
        let raw = crate::recording::tty_termios_of(&pty.path).expect("readable pty");
        assert_eq!(raw.c_lflag & (libc::ISIG | libc::ICANON), 0);
        assert_eq!(raw.c_iflag & libc::IXON, 0);

        // Reading left the settings as the program set them.
        let again = crate::recording::tty_termios_of(&pty.path).expect("readable pty");
        assert_eq!((again.c_lflag, again.c_iflag), (raw.c_lflag, raw.c_iflag));
    }

    #[test]
    fn a_path_that_is_not_a_terminal_is_an_error() {
        let error = crate::recording::tty_termios_of("/dev/null").unwrap_err();
        assert!(
            error.contains("failed to read terminal settings"),
            "{error}"
        );
        let error = crate::recording::tty_termios_of("/nonexistent/tty").unwrap_err();
        assert!(error.contains("failed to open"), "{error}");
    }

    #[test]
    fn a_child_on_a_raw_pty_reports_the_childs_settings_not_whykeys() {
        use std::os::unix::process::CommandExt;
        let pty = Pty::open();
        pty.set_flags(0, 0);
        let slave = pty.slave;
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        // SAFETY: only async-signal-safe calls run between the process split
        // and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 || libc::ioctl(slave, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().expect("spawn sleep");
        let result = read(child.id());
        let _ = child.kill();
        let _ = child.wait();

        let target = result.expect("the pty is readable");
        assert_eq!(target.termios.c_lflag & (libc::ISIG | libc::ICANON), 0);
        assert_eq!(target.termios.c_iflag & libc::IXON, 0);
        let evidence = target.details.join("\n");
        assert!(evidence.contains(&pty.path), "{evidence}");
        assert!(evidence.contains("controlling terminal"), "{evidence}");
        assert!(evidence.contains("(sleep)"), "{evidence}");
    }

    #[test]
    fn a_missing_process_is_an_error_not_whykeys_terminal() {
        let error = read(u32::MAX).err().expect("no such process");
        assert!(error.contains("not readable under /proc"), "{error}");
    }
}
