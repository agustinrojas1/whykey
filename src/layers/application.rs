use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::command;
use crate::key::KeyCombo;
use crate::layers::{
    BindingEvidence, BindingScope, LayerId, LayerResult, Outcome, UncertaintyReason,
};

const MAX_ANCESTORS: usize = 16;

pub fn inspect(key: &KeyCombo) -> LayerResult {
    inspect_for_pid_with_source_inner(key, std::process::id(), None, false)
}

pub fn inspect_for_pid(key: &KeyCombo, target_pid: u32) -> LayerResult {
    inspect_for_pid_with_source_inner(key, target_pid, None, true)
}

pub fn inspect_for_pid_with_source(
    key: &KeyCombo,
    target_pid: u32,
    source: Option<&str>,
) -> LayerResult {
    inspect_for_pid_with_source_inner(key, target_pid, source, true)
}

fn inspect_for_pid_with_source_inner(
    key: &KeyCombo,
    target_pid: u32,
    source: Option<&str>,
    is_explicit_target: bool,
) -> LayerResult {
    if is_shell_process(target_pid) {
        let mut details = vec![format!("target pid: {target_pid}")];
        if let Some(source) = source {
            details.push(format!("target selection source: {source}"));
        }
        details.push("selected target is a shell".into());
        return LayerResult::new(
            "Interactive application",
            LayerId::Application,
            Outcome::Pass,
            "selected process is a shell",
            details,
        );
    }

    let Some((name, command, application_pid)) = interactive_ancestor_from(target_pid) else {
        if !is_explicit_target {
            let details = vec![format!("target pid: {target_pid}")];
            return LayerResult::new(
                "Interactive application",
                LayerId::Application,
                Outcome::Pass,
                "no known interactive editor ancestor detected for target process",
                details,
            );
        }
        let Some((_parent, command)) = process_info(target_pid) else {
            let mut details = vec![
                "the process may have exited or /proc access may be restricted".into(),
                "select a live process from the same session".into(),
            ];
            if let Some(source) = source {
                details.push(format!("target selection source: {source}"));
            }
            return LayerResult::new(
                "Interactive application",
                LayerId::Application,
                Outcome::Unavailable,
                format!("target process {target_pid} is unavailable"),
                details,
            )
            .with_binding(BindingEvidence {
                dispatcher: None,
                action: None,
                description: None,
                submap: None,
                scope: BindingScope::Unknown,
                source: None,
                has_universal_match: false,
                uncertainty: Some(UncertaintyReason::EndpointUnavailable),
            });
        };
        let mut details = vec![
            format!("target pid: {target_pid}"),
            format!("command: {command}"),
        ];
        if let Some(source) = source {
            details.push(format!("target selection source: {source}"));
        }
        details.push("selected process has no dedicated shortcut inspection adapter".into());
        details.push("foreground terminal ownership is unverified".into());
        let bin_name = Path::new(command.split_whitespace().next().unwrap_or_default())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("process");
        return LayerResult::new(
            "Interactive application",
            LayerId::Application,
            Outcome::UnadaptedTarget,
            format!(
                "selected process '{bin_name}' ({target_pid}) has no dedicated shortcut adapter; handling is unverified"
            ),
            details,
        );
    };

    let mut details = vec![
        format!("target pid: {target_pid}"),
        format!("application pid: {application_pid}"),
        format!("ancestor command: {command}"),
    ];
    if let Some(source) = source {
        details.push(format!("target selection source: {source}"));
    }
    if name == "nvim" {
        if let Some(mapping) = nvim_runtime_mapping(key, &command, application_pid) {
            details.push(format!("runtime mapping: {mapping}"));
            details.push("mapping queried through Neovim RPC".into());
            details.push("foreground terminal ownership is unverified".into());
            let binding = BindingEvidence {
                dispatcher: None,
                action: Some(mapping),
                description: None,
                submap: None,
                scope: BindingScope::Unknown,
                source: None,
                has_universal_match: false,
                uncertainty: Some(UncertaintyReason::UnresolvedMode),
            };
            return LayerResult::new(
                "Neovim",
                LayerId::Application,
                Outcome::HandledUncertain,
                "runtime mapping found; Neovim may consume the key",
                details,
            )
            .with_binding(binding);
        }
        if let Some(mapping) = nvim_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push("Neovim mode and plugin precedence are runtime-dependent".into());
            return LayerResult::new(
                "Neovim",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Neovim may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in Neovim RPC or init.lua/init.vim".into());
        details.push("plugins and runtime mappings were not inspected".into());
        return LayerResult::new(
            "Neovim",
            LayerId::Application,
            Outcome::Unknown,
            "Neovim is active; application handling cannot be proven",
            details,
        );
    }
    if matches!(name.as_str(), "vim" | "vimx" | "vi" | "gvim") {
        if let Some(mapping) = vim_runtime_mapping(key, &command) {
            details.push(format!("runtime mapping: {mapping}"));
            details.push("mapping queried through Vim remote expression".into());
            return LayerResult::new(
                "Vim",
                LayerId::Application,
                Outcome::HandledUncertain,
                "runtime mapping found; Vim may consume the key",
                details,
            );
        }
        if let Some(mapping) = vim_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push("Vim mode and plugin precedence are runtime-dependent".into());
            return LayerResult::new(
                "Vim",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Vim may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in Vim RPC or vimrc".into());
        details.push("plugins and runtime mappings were not inspected".into());
        return LayerResult::new(
            "Vim",
            LayerId::Application,
            Outcome::Unknown,
            "Vim is active; application handling cannot be proven",
            details,
        );
    }
    if matches!(name.as_str(), "emacs" | "emacsclient") {
        if let Some(mapping) = emacs_runtime_mapping(key) {
            details.push(format!("runtime mapping: {mapping}"));
            details.push("mapping queried through emacsclient key-binding".into());
            return LayerResult::new(
                "Emacs",
                LayerId::Application,
                Outcome::HandledUncertain,
                "runtime mapping found; Emacs may consume the key",
                details,
            );
        }
        if let Some(mapping) = emacs_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details
                .push("Emacs major/minor mode and keymap precedence are runtime-dependent".into());
            return LayerResult::new(
                "Emacs",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Emacs may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in Emacs init files".into());
        details.push("major/minor mode and package mappings were not inspected".into());
        return LayerResult::new(
            "Emacs",
            LayerId::Application,
            Outcome::Unknown,
            "Emacs is active; application handling cannot be proven",
            details,
        );
    }
    if matches!(name.as_str(), "helix" | "hx") {
        if let Some(mapping) = helix_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push("Helix mode and runtime/plugin precedence are conditional".into());
            return LayerResult::new(
                "Helix",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Helix may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in Helix config.toml".into());
        details.push("mode and runtime/plugin mappings were not inspected".into());
        return LayerResult::new(
            "Helix",
            LayerId::Application,
            Outcome::Unknown,
            "Helix is active; application handling cannot be proven",
            details,
        );
    }
    if name == "micro" {
        if let Some(mapping) = micro_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push("Micro mode and plugin precedence are conditional".into());
            return LayerResult::new(
                "Micro",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Micro may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in Micro bindings.json".into());
        details.push("plugin and mode mappings were not inspected".into());
        return LayerResult::new(
            "Micro",
            LayerId::Application,
            Outcome::Unknown,
            "Micro is active; application handling cannot be proven",
            details,
        );
    }
    if matches!(name.as_str(), "kak" | "kakoune") {
        if let Some(mapping) = kakoune_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push("Kakoune context and runtime/plugin precedence are conditional".into());
            return LayerResult::new(
                "Kakoune",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; Kakoune may consume the key",
                details,
            );
        }
        details.push("no matching mapping found in kakrc".into());
        details.push("runtime and plugin mappings were not inspected".into());
        return LayerResult::new(
            "Kakoune",
            LayerId::Application,
            Outcome::Unknown,
            "Kakoune is active; application handling cannot be proven",
            details,
        );
    }
    if matches!(name.as_str(), "code" | "code-oss" | "codium" | "vscodium") {
        if let Some(mapping) = vscode_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push(
                "VS Code keybinding context, chords, and extension precedence are conditional"
                    .into(),
            );
            return LayerResult::new(
                "VS Code",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; VS Code may consume the key",
                details,
            );
        }
        details.push("no matching single-key mapping found in VS Code keybindings.json".into());
        details.push(
            "chords, `when` clauses, extensions, and runtime context were not inspected".into(),
        );
        return LayerResult::new(
            "VS Code",
            LayerId::Application,
            Outcome::Unknown,
            "VS Code is active; application handling cannot be proven",
            details,
        );
    }
    if is_jetbrains_name(&name) {
        if let Some(mapping) = jetbrains_static_mapping(key) {
            details.push(format!("mapping: {mapping}"));
            details.push(
                "JetBrains keymap context, plugins, and IDE action precedence are conditional"
                    .into(),
            );
            return LayerResult::new(
                "JetBrains IDE",
                LayerId::Application,
                Outcome::HandledUncertain,
                "mapping found; JetBrains may consume the key",
                details,
            );
        }
        details.push("no matching shortcut found in the selected JetBrains keymap".into());
        details
            .push("IDE context, plugins, and runtime keymap overrides were not inspected".into());
        return LayerResult::new(
            "JetBrains IDE",
            LayerId::Application,
            Outcome::Unknown,
            "JetBrains IDE is active; application handling cannot be proven",
            details,
        );
    }
    details.push("application shortcut mappings are not inspected".into());
    details.push("foreground terminal ownership is unverified".into());
    LayerResult::new(
        "Selected application",
        LayerId::Application,
        Outcome::Unknown,
        format!("selected application '{name}' may consume the key before the shell"),
        details,
    )
}

fn interactive_ancestor_from(start_pid: u32) -> Option<(String, String, u32)> {
    let mut pid = start_pid;
    for _ in 0..MAX_ANCESTORS {
        let (parent, command) = process_info(pid)?;
        let name = Path::new(command.split_whitespace().next()?)
            .file_name()?
            .to_string_lossy()
            .to_ascii_lowercase();
        if is_interactive_editor(&name) {
            return Some((name, command, pid));
        }
        if parent == 0 || parent == pid {
            break;
        }
        pid = parent;
    }
    None
}

pub fn is_shell_process(pid: u32) -> bool {
    let Some((_, command)) = process_info(pid) else {
        return false;
    };
    let Some(name) = Path::new(command.split_whitespace().next().unwrap_or_default())
        .file_name()
        .and_then(|s| s.to_str())
    else {
        return false;
    };
    let clean_name = name.strip_prefix('-').unwrap_or(name).to_ascii_lowercase();
    matches!(
        clean_name.as_str(),
        "bash" | "zsh" | "fish" | "sh" | "dash" | "tcsh" | "csh" | "ksh"
    )
}

fn process_info(pid: u32) -> Option<(u32, String)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(") ")?;
    let fields: Vec<_> = stat[close + 2..].split_whitespace().collect();
    // After comm, /proc stat fields start at state (field 3), then ppid is
    // field 4, i.e. index 1 in this slice.
    let parent = fields.get(1)?.parse().ok()?;
    let command = fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            bytes
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| {
            stat.find('(')
                .map(|open| stat[open + 1..close].to_owned())
                .unwrap_or_default()
        });
    Some((parent, command))
}

fn process_environment_value(pid: u32, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    fs::read(format!("/proc/{pid}/environ"))
        .ok()?
        .split(|byte| *byte == 0)
        .find_map(|value| {
            let value = std::str::from_utf8(value).ok()?;
            value
                .strip_prefix(&prefix)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_owned)
}

fn is_interactive_editor(name: &str) -> bool {
    matches!(
        name,
        "nvim"
            | "vim"
            | "vimx"
            | "vi"
            | "gvim"
            | "emacs"
            | "emacsclient"
            | "helix"
            | "hx"
            | "kak"
            | "kakoune"
            | "micro"
            | "code"
            | "code-oss"
            | "codium"
            | "vscodium"
            | "idea"
            | "idea.sh"
            | "pycharm"
            | "pycharm.sh"
            | "webstorm"
            | "webstorm.sh"
            | "clion"
            | "clion.sh"
            | "goland"
            | "goland.sh"
            | "rubymine"
            | "rubymine.sh"
            | "phpstorm"
            | "phpstorm.sh"
            | "rider"
            | "rider.sh"
            | "datagrip"
            | "datagrip.sh"
            // Common full-screen terminal applications. Their keymaps are
            // intentionally not guessed here; detecting them prevents the
            // shell layer from being presented as the final consumer.
            | "fzf"
            | "sk"
            | "peco"
            | "less"
            | "most"
            | "nano"
            | "btop"
            | "htop"
            | "lazygit"
            | "tig"
            | "ranger"
            | "lf"
            | "yazi"
            | "mc"
            | "mutt"
            | "neomutt"
            | "weechat"
            | "irssi"
            | "newsboat"
    )
}

fn is_jetbrains_name(name: &str) -> bool {
    matches!(
        name,
        "idea"
            | "idea.sh"
            | "pycharm"
            | "pycharm.sh"
            | "webstorm"
            | "webstorm.sh"
            | "clion"
            | "clion.sh"
            | "goland"
            | "goland.sh"
            | "rubymine"
            | "rubymine.sh"
            | "phpstorm"
            | "phpstorm.sh"
            | "rider"
            | "rider.sh"
            | "datagrip"
            | "datagrip.sh"
    )
}

fn nvim_runtime_mapping(key: &KeyCombo, command: &str, process_pid: u32) -> Option<String> {
    // Prefer the endpoint advertised by the selected process. Falling back to
    // that process's own environment handles wrappers that do not put
    // `--listen` in argv. Never use whykey's environment here: it can point at
    // a different editor when `--pid` targets another process.
    let server = nvim_server_from_command(command)
        .or_else(|| process_environment_value(process_pid, "NVIM_LISTEN_ADDRESS"))
        .or_else(|| process_environment_value(process_pid, "NVIM"))?;
    let notation = nvim_key_notations(key).into_iter().next()?;
    let modes = nvim_current_mode(&server)
        .map(|mode| vec![mode])
        .unwrap_or_else(|| {
            vec![
                "n".into(),
                "i".into(),
                "v".into(),
                "x".into(),
                "s".into(),
                "c".into(),
                "t".into(),
                "o".into(),
            ]
        });
    for mode in modes {
        let expression = format!(
            "maparg({}, {}, 0, 1)",
            vimscript_string(&notation),
            vimscript_string(&mode)
        );
        let mut query = Command::new("nvim");
        query.args([
            "--headless",
            "--server",
            &server,
            "--remote-expr",
            &expression,
        ]);
        let output = match command::output(&mut query) {
            Ok(output) => output,
            Err(_) => continue,
        };
        if output.status.success() {
            if let Some(mapping) = runtime_mapping_output(&output.stdout) {
                return Some(format!("mode {mode}: {mapping}"));
            }
        }
    }
    None
}

fn vim_runtime_mapping(key: &KeyCombo, command: &str) -> Option<String> {
    let server = vim_server_from_command(command)?;
    let notation = nvim_key_notations(key).into_iter().next()?;
    let modes = vim_current_mode(&server)
        .map(|mode| vec![mode])
        .unwrap_or_else(|| {
            vec![
                "n".into(),
                "i".into(),
                "v".into(),
                "x".into(),
                "s".into(),
                "c".into(),
                "t".into(),
                "o".into(),
            ]
        });
    for mode in modes {
        let expression = format!(
            "maparg({}, {}, 0, 1)",
            vimscript_string(&notation),
            vimscript_string(&mode)
        );
        let mut query = Command::new("vim");
        query.args(["--servername", &server, "--remote-expr", &expression]);
        let output = command::output(&mut query).ok()?;
        if output.status.success() {
            if let Some(mapping) = runtime_mapping_output(&output.stdout) {
                return Some(format!("mode {mode}: {mapping}"));
            }
        }
    }
    None
}

fn nvim_current_mode(server: &str) -> Option<String> {
    let mut query = Command::new("nvim");
    query.args(["--headless", "--server", server, "--remote-expr", "mode()"]);
    let output = command::output(&mut query).ok()?;
    output
        .status
        .success()
        .then(|| runtime_mode(&output.stdout))?
}

fn vim_current_mode(server: &str) -> Option<String> {
    let mut query = Command::new("vim");
    query.args(["--servername", server, "--remote-expr", "mode()"]);
    let output = command::output(&mut query).ok()?;
    output
        .status
        .success()
        .then(|| runtime_mode(&output.stdout))?
}

fn runtime_mode(output: &[u8]) -> Option<String> {
    let mode = String::from_utf8_lossy(output)
        .trim()
        .trim_matches('"')
        .chars()
        .next()?;
    matches!(mode, 'n' | 'i' | 'v' | 'x' | 's' | 'c' | 't' | 'o').then(|| mode.to_string())
}

fn vimscript_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn emacs_runtime_mapping(key: &KeyCombo) -> Option<String> {
    let notation = emacs_key_notations(key).into_iter().next()?;
    let expression = format!(
        "(let ((binding (key-binding (kbd {})))) (if binding (symbol-name binding) nil))",
        elisp_string(&notation)
    );
    let mut command = Command::new("emacsclient");
    if let Some(server_file) = std::env::var_os("EMACS_SERVER_FILE") {
        let server_file = server_file.to_string_lossy().into_owned();
        command.args(["--server-file", &server_file]);
    } else if let Some(socket_name) = std::env::var_os("EMACS_SOCKET_NAME") {
        let socket_name = socket_name.to_string_lossy().into_owned();
        command.args(["--socket-name", &socket_name]);
    }
    command.args(["--eval", &expression]);
    let output = command::output(&mut command).ok()?;
    if !output.status.success() {
        return None;
    }
    let mapping = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_matches('"')
        .to_owned();
    (!mapping.is_empty() && mapping != "nil").then_some(mapping)
}

fn elisp_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

fn runtime_mapping_output(output: &[u8]) -> Option<String> {
    let mapping = String::from_utf8_lossy(output).trim().to_owned();
    (!mapping.is_empty() && mapping != "{}" && mapping != "{ }").then_some(mapping)
}

fn vim_server_from_command(command: &str) -> Option<String> {
    let arguments: Vec<_> = command.split('\0').collect();
    let arguments = if arguments.len() == 1 {
        command.split_whitespace().collect()
    } else {
        arguments
    };
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if let Some(server) = argument.strip_prefix("--servername=") {
            return (!server.is_empty()).then_some(server.to_owned());
        }
        (argument == &"--servername".to_owned())
            .then(|| {
                arguments
                    .get(index + 1)
                    .copied()
                    .unwrap_or_default()
                    .to_owned()
            })
            .filter(|server| !server.is_empty())
    })
}

pub fn nvim_server_from_command(command: &str) -> Option<String> {
    let arguments: Vec<_> = command.split('\0').collect();
    let arguments = if arguments.len() == 1 {
        command.split_whitespace().collect()
    } else {
        arguments
    };
    arguments.iter().enumerate().find_map(|(index, argument)| {
        if let Some(address) = argument.strip_prefix("--listen=") {
            return (!address.is_empty()).then_some(address.to_owned());
        }
        (argument == &"--listen".to_owned())
            .then(|| {
                arguments
                    .get(index + 1)
                    .copied()
                    .unwrap_or_default()
                    .to_owned()
            })
            .filter(|address| !address.is_empty())
    })
}

fn nvim_static_mapping(key: &KeyCombo) -> Option<String> {
    static_mapping(nvim_config_paths(), nvim_key_notations(key), |line| {
        line.contains("keymap")
            || line.contains("set_keymap")
            || line
                .split_whitespace()
                .next()
                .is_some_and(|command| command.ends_with("map") || command.ends_with("map!"))
    })
}

fn vim_static_mapping(key: &KeyCombo) -> Option<String> {
    static_mapping(vim_config_paths(), nvim_key_notations(key), |line| {
        line.split_whitespace().next().is_some_and(|command| {
            matches!(
                command,
                "map"
                    | "nmap"
                    | "imap"
                    | "vmap"
                    | "xmap"
                    | "cmap"
                    | "omap"
                    | "map!"
                    | "nnoremap"
                    | "inoremap"
                    | "vnoremap"
                    | "xnoremap"
                    | "cnoremap"
            )
        })
    })
}

fn emacs_static_mapping(key: &KeyCombo) -> Option<String> {
    let notations = emacs_key_notations(key);
    let paths = emacs_config_paths();
    paths.into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        content.lines().find_map(|raw_line| {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(';') {
                return None;
            }
            let lower = line.to_ascii_lowercase();
            let command = line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_start_matches('(');
            (matches!(command, "global-set-key" | "define-key")
                && notations
                    .iter()
                    .any(|notation| lower.contains(&notation.to_ascii_lowercase())))
            .then(|| format!("{}: {line}", path.display()))
        })
    })
}

fn helix_static_mapping(key: &KeyCombo) -> Option<String> {
    helix_config_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        let mut in_keys = false;
        for raw_line in content.lines() {
            let line = raw_line.split('#').next()?.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(section) = line
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
            {
                in_keys = section == "keys"
                    || section
                        .strip_prefix("keys.")
                        .is_some_and(|mode| !mode.is_empty());
                continue;
            }
            if !in_keys {
                continue;
            }
            let Some((raw_key, _)) = line.split_once('=') else {
                continue;
            };
            let raw_key = raw_key.trim().trim_matches(['"', '\'']);
            if parse_helix_key(raw_key).as_ref() == Some(key) {
                return Some(format!("{}: {line}", path.display()));
            }
        }
        None
    })
}

fn micro_static_mapping(key: &KeyCombo) -> Option<String> {
    micro_config_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        let object = value.as_object()?;
        object.iter().find_map(|(raw_key, action)| {
            (parse_micro_key(raw_key).as_ref() == Some(key)).then(|| {
                format!(
                    "{}: {raw_key} = {}",
                    path.display(),
                    action.as_str().unwrap_or(&action.to_string())
                )
            })
        })
    })
}

fn kakoune_static_mapping(key: &KeyCombo) -> Option<String> {
    kakoune_config_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        content.lines().find_map(|raw_line| {
            let line = raw_line.split('#').next()?.trim();
            let mut fields = line.split_whitespace();
            if fields.next()? != "map" {
                return None;
            }
            fields.next()?;
            fields.next()?;
            let raw_key = fields.next()?;
            (parse_kakoune_key(raw_key).as_ref() == Some(key))
                .then(|| format!("{}: {line}", path.display()))
        })
    })
}

fn vscode_static_mapping(key: &KeyCombo) -> Option<String> {
    vscode_config_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        parse_vscode_mapping(&content, key).map(|mapping| format!("{}: {mapping}", path.display()))
    })
}

fn parse_vscode_mapping(content: &str, key: &KeyCombo) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    value.as_array()?.iter().find_map(|entry| {
        let object = entry.as_object()?;
        let raw_key = object.get("key")?.as_str()?;
        if raw_key.contains(char::is_whitespace) || raw_key.parse::<KeyCombo>().ok()?.ne(key) {
            return None;
        }
        let command = object.get("command")?.as_str()?;
        let when = object.get("when").and_then(serde_json::Value::as_str);
        Some(match when.filter(|when| !when.trim().is_empty()) {
            Some(when) => format!("{raw_key} -> {command} (when: {when})"),
            None => format!("{raw_key} -> {command}"),
        })
    })
}

fn jetbrains_static_mapping(key: &KeyCombo) -> Option<String> {
    jetbrains_config_paths().into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        parse_jetbrains_mapping(&content, key)
            .map(|mapping| format!("{}: {mapping}", path.display()))
    })
}

fn parse_jetbrains_mapping(content: &str, key: &KeyCombo) -> Option<String> {
    let mut offset = 0;
    while let Some(relative) = content[offset..].find("<keyboard-shortcut") {
        let start = offset + relative;
        let end = content[start..].find('>')? + start;
        let tag = &content[start..=end];
        let Some(raw_key) = xml_attribute(tag, "first-keystroke")
            .or_else(|| xml_attribute(tag, "second-keystroke"))
        else {
            offset = end + 1;
            continue;
        };
        let Some(parsed) = parse_jetbrains_key(&raw_key) else {
            offset = end + 1;
            continue;
        };
        if &parsed == key {
            let prefix = &content[..start];
            let action = prefix
                .rfind("<action")
                .and_then(|action_start| {
                    let action_end = prefix[action_start..].find('>')? + action_start;
                    xml_attribute(&prefix[action_start..=action_end], "id")
                })
                .unwrap_or_else(|| "unknown action".into());
            return Some(format!("{raw_key} -> {action}"));
        }
        offset = end + 1;
    }
    None
}

fn parse_jetbrains_key(value: &str) -> Option<KeyCombo> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for token in value.split_whitespace() {
        let normalized = match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some("ctrl"),
            "alt" => Some("alt"),
            "shift" => Some("shift"),
            "meta" | "cmd" | "command" => Some("super"),
            _ => None,
        };
        if let Some(modifier) = normalized {
            modifiers.push(modifier);
        } else if key.is_some() {
            return None;
        } else {
            key = Some(token.to_owned());
        }
    }
    combo_from_key_parts(&modifiers, key?)
}

fn combo_from_key_parts(modifiers: &[&str], key: String) -> Option<KeyCombo> {
    let mut value = modifiers.join("+");
    if !value.is_empty() {
        value.push('+');
    }
    value.push_str(&key);
    value.parse().ok()
}

fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')?;
    Some(tag[start..start + end].to_owned())
}

fn static_mapping<F>(paths: Vec<PathBuf>, notations: Vec<String>, predicate: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    paths.into_iter().find_map(|path| {
        let content = fs::read_to_string(&path).ok()?;
        content.lines().find_map(|raw_line| {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('"') || line.starts_with("--") {
                return None;
            }
            (predicate(line) && notations.iter().any(|notation| line.contains(notation)))
                .then(|| format!("{}: {line}", path.display()))
        })
    })
}

fn vscode_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("VSCODE_KEYBINDINGS") {
        paths.push(PathBuf::from(path));
    }
    let Some(base) = crate::util::config_base() else {
        return paths;
    };
    for application in ["Code", "Code - OSS", "VSCodium"] {
        paths.push(base.join(application).join("User/keybindings.json"));
    }
    paths
}

fn jetbrains_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("JETBRAINS_KEYMAP") {
        paths.push(PathBuf::from(path));
    }
    let Some(base) = crate::util::config_base() else {
        return paths;
    };
    let root = base.join("JetBrains");
    let Ok(products) = fs::read_dir(root) else {
        return paths;
    };
    for product in products.flatten().take(32) {
        let keymaps = product.path().join("keymaps");
        let Ok(files) = fs::read_dir(keymaps) else {
            continue;
        };
        for file in files.flatten().take(64) {
            let path = file.path();
            if path.extension().is_some_and(|extension| extension == "xml") {
                paths.push(path);
            }
        }
    }
    paths
}

fn nvim_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("MYVIMRC") {
        paths.push(PathBuf::from(path));
    }
    let app_name = std::env::var_os("NVIM_APPNAME").unwrap_or_else(|| "nvim".into());
    if let Some(app_base) = crate::util::config_base().map(|base| base.join(app_name)) {
        paths.push(app_base.join("init.lua"));
        paths.push(app_base.join("init.vim"));
    }
    paths
}

fn vim_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("MYVIMRC") {
        paths.push(PathBuf::from(path));
    }
    if let Some(base) = crate::util::config_base() {
        let base = base.join("vim");
        paths.push(base.join("vimrc"));
        paths.push(base.join(".vimrc"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".vimrc"));
        paths.push(home.join(".vim/vimrc"));
    }
    paths
}

fn emacs_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("emacs/early-init.el"));
        paths.push(base.join("emacs/init.el"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".emacs"));
        paths.push(home.join(".emacs.d/early-init.el"));
        paths.push(home.join(".emacs.d/init.el"));
        paths.push(home.join(".config/emacs/early-init.el"));
        paths.push(home.join(".config/emacs/init.el"));
    }
    paths
}

fn helix_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("helix/config.toml"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".config/helix/config.toml"));
    }
    paths
}

fn micro_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(base) = std::env::var_os("MICRO_CONFIG_HOME") {
        paths.push(PathBuf::from(base).join("bindings.json"));
    }
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("micro/bindings.json"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".config/micro/bindings.json"));
    }
    paths.push(PathBuf::from("/etc/micro/bindings.json"));
    paths
}

fn kakoune_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(base) = crate::util::config_base() {
        paths.push(base.join("kak/kakrc"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".config/kak/kakrc"));
        paths.push(home.join(".kakrc"));
    }
    paths
}

fn parse_helix_key(raw: &str) -> Option<KeyCombo> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in raw.split('-') {
        match part.to_ascii_lowercase().as_str() {
            "c" | "ctrl" | "control" => modifiers.push("ctrl"),
            "a" | "alt" | "meta" => modifiers.push("alt"),
            "s" | "shift" => modifiers.push("shift"),
            "g" | "super" => modifiers.push("super"),
            value if key.is_none() => key = Some(value.to_owned()),
            _ => return None,
        }
    }
    let key = key?;
    let key = match key.as_str() {
        "ret" | "return" => "return",
        "esc" | "escape" => "escape",
        "space" => "space",
        "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown" => key.as_str(),
        _ => key.as_str(),
    };
    modifiers.push(key);
    modifiers.join("+").parse().ok()
}

fn parse_micro_key(raw: &str) -> Option<KeyCombo> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in raw.split('-') {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.push("ctrl"),
            "alt" | "meta" => modifiers.push("alt"),
            "shift" => modifiers.push("shift"),
            "cmd" | "super" => modifiers.push("super"),
            value if key.is_none() => key = Some(value.to_owned()),
            _ => return None,
        }
    }
    let key = key?;
    modifiers.push(key.as_str());
    modifiers.join("+").parse().ok()
}

fn parse_kakoune_key(raw: &str) -> Option<KeyCombo> {
    let inner = raw.strip_prefix('<')?.strip_suffix('>')?;
    let mut modifiers = Vec::new();
    let mut key = None;
    for part in inner.split('-') {
        match part.to_ascii_lowercase().as_str() {
            "c" | "ctrl" => modifiers.push("ctrl"),
            "a" | "alt" => modifiers.push("alt"),
            "s" | "shift" => modifiers.push("shift"),
            "g" | "super" => modifiers.push("super"),
            value if key.is_none() => key = Some(value.to_owned()),
            _ => return None,
        }
    }
    let key = key?;
    modifiers.push(key.as_str());
    modifiers.join("+").parse().ok()
}

fn emacs_key_notations(key: &KeyCombo) -> Vec<String> {
    let mut notation = String::new();
    if key.modmask() & 4 != 0 {
        notation.push_str("C-");
    }
    if key.modmask() & 8 != 0 {
        notation.push_str("M-");
    }
    if key.modmask() & 1 != 0 {
        notation.push_str("S-");
    }
    let name = match key.key() {
        "LEFT" => "<left>",
        "RIGHT" => "<right>",
        "UP" => "<up>",
        "DOWN" => "<down>",
        "RETURN" => "RET",
        "ESCAPE" => "ESC",
        "SPACE" => "SPC",
        "TAB" => "TAB",
        value => value,
    };
    notation.push_str(name);
    vec![notation.clone(), format!("\\{notation}")]
}

fn nvim_key_notations(key: &KeyCombo) -> Vec<String> {
    let mut notation = String::from("<");
    if key.modmask() & 4 != 0 {
        notation.push_str("C-");
    }
    if key.modmask() & 8 != 0 {
        notation.push_str("M-");
    }
    if key.modmask() & 1 != 0 {
        notation.push_str("S-");
    }
    let name = match key.key() {
        "LEFT" => "Left",
        "RIGHT" => "Right",
        "UP" => "Up",
        "DOWN" => "Down",
        "RETURN" => "CR",
        "ESCAPE" => "Esc",
        "SPACE" => "Space",
        "TAB" => "Tab",
        value => value,
    };
    notation.push_str(name);
    notation.push('>');
    vec![notation]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_interactive_editors() {
        assert!(is_interactive_editor("nvim"));
        assert!(is_interactive_editor("helix"));
        assert!(is_interactive_editor("vim"));
        assert!(is_interactive_editor("fzf"));
        assert!(is_interactive_editor("lazygit"));
        assert!(is_interactive_editor("code"));
        assert!(is_interactive_editor("pycharm"));
        assert!(!is_interactive_editor("bash"));
    }

    #[test]
    fn formats_neovim_key_notation() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        assert_eq!(nvim_key_notations(&key), vec!["<C-Left>"]);
    }

    #[test]
    fn formats_emacs_key_notations() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        assert_eq!(emacs_key_notations(&key), vec!["C-<left>", "\\C-<left>"]);
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        assert!(emacs_key_notations(&key).contains(&"C-F".into()));
    }

    #[test]
    fn parses_parent_from_proc_stat_shape() {
        let fields = "S 1234 1 2".split_whitespace().collect::<Vec<_>>();
        assert_eq!(
            fields.get(1).and_then(|value| value.parse::<u32>().ok()),
            Some(1234)
        );
    }

    #[test]
    fn finds_neovim_listen_address_in_command_line() {
        assert_eq!(
            nvim_server_from_command("nvim --clean --listen /tmp/nvim.sock"),
            Some("/tmp/nvim.sock".into())
        );
        assert_eq!(
            nvim_server_from_command("nvim --headless --listen=/tmp/nvim.sock"),
            Some("/tmp/nvim.sock".into())
        );
        assert_eq!(nvim_server_from_command("nvim --clean"), None);
    }

    #[test]
    fn finds_vim_server_name_in_command_line() {
        assert_eq!(
            vim_server_from_command("vim --servername VIM --clean"),
            Some("VIM".into())
        );
        assert_eq!(
            vim_server_from_command("gvim --servername=EDITOR"),
            Some("EDITOR".into())
        );
        assert_eq!(vim_server_from_command("vim --clean"), None);
    }

    #[test]
    fn escapes_vimscript_string_literals() {
        assert_eq!(vimscript_string("<C-'>"), "'<C-''>'");
        assert_eq!(vimscript_string("n"), "'n'");
    }

    #[test]
    fn escapes_elisp_string_literals() {
        assert_eq!(elisp_string("C-<left>"), "\"C-<left>\"");
        assert_eq!(elisp_string("C-\\\""), "\"C-\\\\\\\"\"");
    }

    #[test]
    fn parses_editor_runtime_modes() {
        assert_eq!(runtime_mode(b"n\n"), Some("n".into()));
        assert_eq!(runtime_mode(b"\"i\"\n"), Some("i".into()));
        assert_eq!(runtime_mode(b"operator pending\n"), Some("o".into()));
        assert_eq!(runtime_mode(b"unknown\n"), None);
    }

    #[test]
    fn parses_helix_key_notation() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        assert_eq!(parse_helix_key("C-left"), Some(key));
    }

    #[test]
    fn parses_micro_key_notation() {
        let key: KeyCombo = "ctrl+q".parse().unwrap();
        assert_eq!(parse_micro_key("Ctrl-q"), Some(key));
    }

    #[test]
    fn parses_kakoune_key_notation() {
        let key: KeyCombo = "ctrl+left".parse().unwrap();
        assert_eq!(parse_kakoune_key("<c-left>"), Some(key));
    }

    #[test]
    fn parses_vscode_single_keybinding_and_when_clause() {
        let key: KeyCombo = "ctrl+shift+p".parse().unwrap();
        let mapping = parse_vscode_mapping(
            r#"[{"key":"ctrl+shift+p","command":"workbench.action.showCommands","when":"editorTextFocus"}]"#,
            &key,
        )
        .unwrap();
        assert!(mapping.contains("workbench.action.showCommands"));
        assert!(mapping.contains("editorTextFocus"));
    }
    #[test]
    fn ignores_generated_vscode_keybindings_without_literal_keys() {
        let key: KeyCombo = "ctrl+shift+p".parse().unwrap();
        assert_eq!(
            parse_vscode_mapping(r#"{"generated":true}"#, &key),
            None,
            "a non-array generated file never becomes a false positive mapping"
        );
        assert_eq!(
            parse_vscode_mapping(
                r#"[{"key":"ctrl+${dynamic}","command":"generated.command"}]"#,
                &key
            ),
            None,
            "a dynamic key expression never becomes a false positive mapping"
        );
    }

    #[test]
    fn parses_jetbrains_keymap_shortcut() {
        let key: KeyCombo = "ctrl+shift+p".parse().unwrap();
        let mapping = parse_jetbrains_mapping(
            r#"<keymap><action id="ShowIntentionActions"><keyboard-shortcut first-keystroke="ctrl shift P" /></action></keymap>"#,
            &key,
        )
        .unwrap();
        assert!(mapping.contains("ShowIntentionActions"));
    }

    #[test]
    fn identifies_jetbrains_process_names() {
        assert!(is_jetbrains_name("idea"));
        assert!(is_interactive_editor("code"));
        assert!(!is_jetbrains_name("code"));
    }
}
