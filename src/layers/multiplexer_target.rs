//! The multiplexer context of an explicit target (`--pid`, `--focused`).
//!
//! The session layer reads the multiplexer from the environment variables of
//! whykey's own process. For another process those say nothing about where
//! its keys go: a target inside a tmux pane started from a plain shell would
//! read "no multiplexer", and one in another pane of whykey's own tmux would
//! be answered for whykey's pane. The target's own environment names its
//! tmux server and pane; a `tmux: client` process (what a focused terminal
//! runs in the foreground) is looked up on its server instead.

use std::collections::HashMap;
use std::process::Command;

use crate::environment::Environment;

/// Replace the multiplexer fields of `env` with those of process `pid`.
pub(crate) fn retarget(env: &mut Environment, pid: u32) {
    env.tmux = false;
    env.screen = false;
    env.zellij = false;
    env.tmux_pane = None;
    env.tmux_server_args.clear();
    env.screen_session = None;
    env.zellij_pane = None;
    env.zellij_session = None;
    env.session_target_note = None;

    let Ok(raw) = crate::recording::read(format!("/proc/{pid}/environ")) else {
        env.session_target_note = Some(format!(
            "the environment of process {pid} is not readable, so whether it runs inside a multiplexer is unknown"
        ));
        refresh_names(env);
        return;
    };
    let vars = parse_environ(&raw);
    if let Some(tmux) = vars.get("TMUX").filter(|value| !value.is_empty()) {
        env.tmux = true;
        env.tmux_pane = vars.get("TMUX_PANE").cloned();
        // `$TMUX` is `socket-path,server-pid,session-index`.
        if let Some(socket) = tmux.split(',').next().filter(|path| !path.is_empty()) {
            env.tmux_server_args = vec!["-S".into(), socket.to_owned()];
        }
    }
    if let Some(session) = vars.get("STY").filter(|value| !value.is_empty()) {
        env.screen = true;
        env.screen_session = Some(session.clone());
    }
    if vars.contains_key("ZELLIJ") || vars.contains_key("ZELLIJ_SESSION_NAME") {
        env.zellij = true;
        env.zellij_pane = vars.get("ZELLIJ_PANE_ID").cloned();
        env.zellij_session = vars.get("ZELLIJ_SESSION_NAME").cloned();
    }
    if !env.tmux {
        if let Some((args, pane)) = tmux_client_pane(pid) {
            env.tmux = true;
            env.tmux_server_args = args;
            env.tmux_pane = Some(pane);
        }
    }
    refresh_names(env);
}

fn refresh_names(env: &mut Environment) {
    let mut names = Vec::new();
    if env.tmux {
        names.push("tmux");
    }
    if env.screen {
        names.push("screen");
    }
    if env.zellij {
        names.push("zellij");
    }
    env.multiplexer = if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(" + ")
    };
}

fn parse_environ(raw: &[u8]) -> HashMap<String, String> {
    raw.split(|byte| *byte == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (name, value) = entry.split_once('=')?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect()
}

/// A `tmux: client` process: the server it talks to (from its `-L`/`-S`
/// arguments) and the pane that client has in front.
fn tmux_client_pane(pid: u32) -> Option<(Vec<String>, String)> {
    let comm = crate::recording::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    if !comm.trim().starts_with("tmux") {
        return None;
    }
    let cmdline = crate::recording::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args = server_args_from_cmdline(&cmdline);
    let mut command = Command::new("tmux");
    command.args(&args);
    command.args(["list-clients", "-F", "#{client_pid}\t#{pane_id}"]);
    let output = crate::command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let pane = pane_of_client(&String::from_utf8_lossy(&output.stdout), pid)?;
    Some((args, pane))
}

fn server_args_from_cmdline(cmdline: &[u8]) -> Vec<String> {
    let words: Vec<String> = cmdline
        .split(|byte| *byte == 0)
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    let mut args = Vec::new();
    let mut index = 1;
    while index < words.len() {
        match words[index].as_str() {
            flag @ ("-L" | "-S") if index + 1 < words.len() => {
                args.push(flag.to_owned());
                args.push(words[index + 1].clone());
                index += 2;
            }
            _ => index += 1,
        }
    }
    args
}

fn pane_of_client(listing: &str, pid: u32) -> Option<String> {
    listing.lines().find_map(|line| {
        let (client, pane) = line.split_once('\t')?;
        (client.trim().parse::<u32>().ok()? == pid && !pane.trim().is_empty())
            .then(|| pane.trim().to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environ_is_split_into_variables() {
        let vars = parse_environ(b"HOME=/root\0TMUX=/tmp/tmux-1000/default,12,0\0TMUX_PANE=%3\0\0");
        assert_eq!(vars["TMUX_PANE"], "%3");
        assert_eq!(vars["TMUX"], "/tmp/tmux-1000/default,12,0");
    }

    #[test]
    fn a_tmux_clients_server_comes_from_its_arguments() {
        assert_eq!(
            server_args_from_cmdline(b"tmux\0-L\0work\0attach\0"),
            ["-L", "work"]
        );
        assert_eq!(
            server_args_from_cmdline(b"tmux\0-S\0/tmp/s\0new\0-A\0"),
            ["-S", "/tmp/s"]
        );
        assert!(server_args_from_cmdline(b"tmux\0attach\0").is_empty());
    }

    fn child_with_env(vars: &[(&str, &str)]) -> std::process::Child {
        let mut command = Command::new("sleep");
        command.arg("30").env_clear();
        command.envs(vars.iter().copied());
        command.spawn().expect("spawn sleep")
    }

    #[test]
    fn a_target_inside_tmux_brings_its_own_server_and_pane() {
        let mut child =
            child_with_env(&[("TMUX", "/tmp/tmux-1000/work,4242,1"), ("TMUX_PANE", "%9")]);
        let mut env = Environment::collect();
        // whykey's own session must not leak into the target's answer.
        env.tmux = false;
        env.tmux_pane = Some("%0".into());
        retarget(&mut env, child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(env.tmux);
        assert_eq!(env.tmux_pane.as_deref(), Some("%9"));
        assert_eq!(env.tmux_server_args, ["-S", "/tmp/tmux-1000/work"]);
        assert_eq!(env.multiplexer, "tmux");
    }

    #[test]
    fn a_target_outside_every_multiplexer_clears_whykeys_own() {
        let mut child = child_with_env(&[("HOME", "/root")]);
        let mut env = Environment::collect();
        env.tmux = true;
        env.tmux_pane = Some("%0".into());
        env.screen = true;
        retarget(&mut env, child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert!(!env.tmux && !env.screen && !env.zellij);
        assert_eq!(env.tmux_pane, None);
        assert_eq!(env.multiplexer, "none");
        assert_eq!(env.session_target_note, None);
    }

    #[test]
    fn an_unreadable_target_is_said_not_assumed() {
        let mut env = Environment::collect();
        env.tmux = true;
        retarget(&mut env, u32::MAX - 1);
        assert!(!env.tmux);
        assert!(env.session_target_note.is_some());
        let layer =
            crate::layers::session::inspect_with_env(&[], &"ctrl+x".parse().unwrap(), true, &env);
        assert_eq!(layer.outcome, crate::layers::Outcome::UncertainContinues);
    }

    #[test]
    fn the_client_row_names_its_pane() {
        let listing = "100\t%1\n200\t%7\n";
        assert_eq!(pane_of_client(listing, 200).as_deref(), Some("%7"));
        assert_eq!(pane_of_client(listing, 300), None);
    }
}
