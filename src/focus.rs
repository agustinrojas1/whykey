use crate::environment::Environment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedTarget {
    pub pid: u32,
    pub source: String,
}

pub fn resolve() -> Result<FocusedTarget, String> {
    let environment = Environment::collect();
    resolve_with_environment(&environment)
}

pub fn resolve_with_environment(environment: &Environment) -> Result<FocusedTarget, String> {
    if let Some(id) = environment.selected_compositor {
        if let Some(entry) = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == id)
        {
            if let Some((source, focused_pid)) = entry.focused_pid.or(entry.focus) {
                if let Ok(pid) = focused_pid() {
                    return Ok(through_terminal(FocusedTarget {
                        pid,
                        source: source.to_owned(),
                    }));
                }
            }
        }
    }
    if let Some(adapter) = environment.extension_adapters.iter().find(|adapter| {
        crate::extension_adapters::applicable_with_context(adapter, &environment.compositor_context)
            && adapter.focused_cmd.is_some()
    }) {
        return crate::extension_adapters::focused_pid(adapter).map(|pid| {
            through_terminal(FocusedTarget {
                pid,
                source: format!("{} (manifest)", adapter.display),
            })
        });
    }
    if environment.selected_compositor.is_none() {
        return Err(
            "no supported compositor session was detected for focused-window discovery".into(),
        );
    }
    Err("the detected desktop does not expose a read-only focused-window PID API".into())
}

/// A focused terminal emulator is not what reads the key: the program in the
/// foreground of its pseudo-terminal is (a shell, an editor). The emulator
/// has no controlling terminal itself, while the children it started on its
/// ptys do; `/proc/<child>/stat` names the pty's foreground process group.
/// When the emulator has exactly one pty, follow it to that group's leader.
/// With several ptys (tabs, windows of one server process) the focused one
/// is unknown, so the target stays the emulator and the source says why.
fn through_terminal(target: FocusedTarget) -> FocusedTarget {
    let Some(own) = proc_stat(target.pid) else {
        return target;
    };
    if own.tty != 0 {
        return target;
    }
    let children: Vec<ProcStat> = child_pids(target.pid)
        .into_iter()
        .filter_map(proc_stat)
        .collect();
    match foreground_choice(&children) {
        ForegroundChoice::NotATerminal => target,
        ForegroundChoice::Process(pid) => FocusedTarget {
            pid,
            source: format!(
                "{}; foreground process of pid {}'s terminal",
                target.source, target.pid
            ),
        },
        ForegroundChoice::Ambiguous(ttys) => FocusedTarget {
            source: format!(
                "{}; pid {} serves {ttys} terminals and the focused one is unknown, so its \
                 shell and applications are not inspected",
                target.source, target.pid
            ),
            ..target
        },
    }
}

/// Whether `pid` has the same controlling terminal as whykey. False when
/// either has none or `/proc` cannot tell.
pub fn shares_terminal_with_whykey(pid: u32) -> bool {
    match (proc_stat(pid), proc_stat(std::process::id())) {
        (Some(target), Some(own)) => own.tty != 0 && target.tty == own.tty,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProcStat {
    pid: u32,
    /// `tty_nr`: the controlling terminal's device number, 0 for none.
    tty: u64,
    /// `tpgid`: foreground process group of the controlling terminal.
    foreground_group: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum ForegroundChoice {
    NotATerminal,
    Process(u32),
    Ambiguous(usize),
}

fn foreground_choice(children: &[ProcStat]) -> ForegroundChoice {
    let on_tty: Vec<&ProcStat> = children.iter().filter(|child| child.tty != 0).collect();
    let mut ttys: Vec<u64> = on_tty.iter().map(|child| child.tty).collect();
    ttys.sort_unstable();
    ttys.dedup();
    match (ttys.len(), on_tty.first()) {
        (0, _) | (_, None) => ForegroundChoice::NotATerminal,
        (1, Some(child)) => {
            // Every process on one pty reports the same foreground group.
            let group = on_tty
                .iter()
                .map(|child| child.foreground_group)
                .max()
                .unwrap_or(-1);
            let leader = u32::try_from(group).ok().filter(|group| *group > 0);
            // The group leader may have exited while the group lives on.
            let alive = leader.filter(|leader| proc_stat(*leader).is_some());
            ForegroundChoice::Process(alive.unwrap_or(child.pid))
        }
        (many, _) => ForegroundChoice::Ambiguous(many),
    }
}

fn proc_stat(pid: u32) -> Option<ProcStat> {
    parse_stat(
        pid,
        &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?,
    )
}

/// Fields after the parenthesized command name (which may contain spaces
/// and parentheses): state ppid pgrp session tty_nr tpgid ...
fn parse_stat(pid: u32, stat: &str) -> Option<ProcStat> {
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    let tty = fields.nth(4)?.parse::<i64>().ok()?;
    let foreground_group = fields.next()?.parse().ok()?;
    Some(ProcStat {
        pid,
        tty: u64::try_from(tty).ok()?,
        foreground_group,
    })
}

fn parent_pid(stat: &str) -> Option<u32> {
    stat.get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn child_pids(parent: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| parent_pid(&stat))
                == Some(parent)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tty_and_foreground_group_after_an_odd_command_name() {
        let stat = "4242 (my (odd) term) S 1 4242 4242 0 -1 4194560 0 0";
        assert_eq!(
            parse_stat(4242, stat),
            Some(ProcStat {
                pid: 4242,
                tty: 0,
                foreground_group: -1
            })
        );
        assert_eq!(parent_pid(stat), Some(1));
        let shell = "4300 (bash) S 4242 4300 4300 34817 4400 4194304 0";
        assert_eq!(
            parse_stat(4300, shell),
            Some(ProcStat {
                pid: 4300,
                tty: 34817,
                foreground_group: 4400
            })
        );
    }

    #[test]
    fn a_terminal_with_one_pty_resolves_to_its_foreground_process() {
        let me = std::process::id();
        let child = |pid, tty, group| ProcStat {
            pid,
            tty,
            foreground_group: group,
        };
        assert_eq!(foreground_choice(&[]), ForegroundChoice::NotATerminal);
        assert_eq!(
            foreground_choice(&[child(10, 0, -1)]),
            ForegroundChoice::NotATerminal
        );
        // The foreground leader (this test process, which exists) wins.
        assert_eq!(
            foreground_choice(&[child(10, 34817, i64::from(me)), child(11, 34817, 0)]),
            ForegroundChoice::Process(me)
        );
        // A vanished leader falls back to the process on that pty.
        assert_eq!(
            foreground_choice(&[child(10, 34817, i64::from(u32::MAX - 1))]),
            ForegroundChoice::Process(10)
        );
        assert_eq!(
            foreground_choice(&[child(10, 34817, 20), child(11, 34818, 21)]),
            ForegroundChoice::Ambiguous(2)
        );
    }

    #[test]
    fn a_process_with_its_own_terminal_is_kept() {
        // This test runs with or without a controlling terminal; either way
        // a target that is not a pty-owning emulator keeps its pid.
        let me = std::process::id();
        let target = through_terminal(FocusedTarget {
            pid: me,
            source: "fixture".into(),
        });
        assert_eq!(target.pid, me);
    }

    #[test]
    fn focused_target_keeps_the_evidence_source() {
        let target = FocusedTarget {
            pid: 42,
            source: "fixture".into(),
        };
        assert_eq!(target.pid, 42);
        assert_eq!(target.source, "fixture");
    }

    #[test]
    fn focus_selection_follows_registry_priority() {
        let mut env = Environment::collect();
        env.selected_compositor = Some("sway");
        let target = crate::registry::DESKTOPS
            .iter()
            .find(|entry| entry.id == env.selected_compositor.unwrap())
            .unwrap();
        assert_eq!(target.id, "sway");

        let hypr_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "hyprland")
            .unwrap();
        let sway_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "sway")
            .unwrap();
        let i3_pos = crate::registry::DESKTOPS
            .iter()
            .position(|e| e.id == "i3")
            .unwrap();
        assert!(hypr_pos < sway_pos);
        assert!(sway_pos < i3_pos);
    }

    #[test]
    fn selected_desktop_without_focus_returns_unsupported_message() {
        let mut env = Environment::collect();
        env.selected_compositor = Some("kde");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );

        env.selected_compositor = Some("gnome");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );

        env.selected_compositor = Some("compositor");
        assert_eq!(
            resolve_with_environment(&env),
            Err("the detected desktop does not expose a read-only focused-window PID API".into())
        );
    }

    #[test]
    fn no_selected_compositor_returns_not_detected_message() {
        let mut env = Environment::collect();
        env.selected_compositor = None;
        assert_eq!(
            resolve_with_environment(&env),
            Err("no supported compositor session was detected for focused-window discovery".into())
        );
    }
}
