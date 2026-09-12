use std::process::ExitCode;
use std::time::Duration;

use whykey::layers::{LayerId, LayerResult, LayerStatus, Outcome, Propagation};
use whykey::schema;
use whykey::style::ColorChoice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobalArguments {
    pub arguments: Vec<String>,
    pub json: bool,
    pub ndjson: bool,
    pub verbose: bool,
    pub color: ColorChoice,
    pub schema_version: u8,
}

/// Remove global presentation flags while preserving command-local arguments.
/// Validation is centralized here so every dispatch path sees the same schema
/// and output-mode rules.
pub(crate) fn parse_global_arguments(raw: Vec<String>) -> Result<GlobalArguments, String> {
    let json = raw
        .iter()
        .any(|argument| matches!(argument.as_str(), "--json" | "--json-v2" | "--ndjson"));
    let ndjson = raw.iter().any(|argument| argument == "--ndjson");
    let verbose = raw
        .iter()
        .any(|argument| matches!(argument.as_str(), "--verbose" | "-v"));
    let mut color = ColorChoice::Auto;
    let mut schema_version = schema::DEFAULT_VERSION;
    let mut arguments = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index].as_str() {
            "--json" | "--ndjson" | "--verbose" | "-v" => {}
            "--color" => {
                let Some(value) = raw.get(index + 1) else {
                    return Err("--color requires auto, always, or never".into());
                };
                color = value.parse()?;
                index += 1;
            }
            option if option.starts_with("--color=") => {
                color = option["--color=".len()..].parse()?;
            }
            "--json-v2" => schema_version = 2,
            "--schema-version" => {
                let Some(value) = raw.get(index + 1) else {
                    return Err("--schema-version requires 1 or 2".into());
                };
                schema_version = match value.parse::<u8>() {
                    Ok(version @ (1 | 2)) => version,
                    _ => return Err("--schema-version must be 1 or 2".into()),
                };
                index += 1;
            }
            argument => arguments.push(argument.to_owned()),
        }
        index += 1;
    }
    Ok(GlobalArguments {
        arguments,
        json,
        ndjson,
        verbose,
        color,
        schema_version,
    })
}

#[derive(Default)]
pub(crate) struct InventoryFilters {
    pub key: Option<String>,
    pub action: Option<String>,
    pub source: Option<String>,
    pub device: Option<String>,
    pub submap: Option<String>,
}

pub(crate) fn parse_inventory_filters(
    arguments: Vec<String>,
    usage: &str,
) -> Result<InventoryFilters, String> {
    let mut filters = InventoryFilters::default();
    let mut arguments = arguments.into_iter();
    while let Some(option) = arguments.next() {
        let target = match option.as_str() {
            "--key" => &mut filters.key,
            "--action" => &mut filters.action,
            "--source" => &mut filters.source,
            "--device" => &mut filters.device,
            "--submap" => &mut filters.submap,
            _ => return Err(format!("usage is `{usage}`")),
        };
        let Some(value) = arguments.next().filter(|value| !value.is_empty()) else {
            return Err(format!("{option} requires a non-empty value"));
        };
        *target = Some(value);
    }
    Ok(filters)
}

/// Take `--option value`, preserving each caller's exact missing-value error.
pub(crate) fn take_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
    noun: &str,
) -> Result<String, ExitCode> {
    match arguments.next() {
        Some(value) => Ok(value),
        None => {
            eprintln!("error: {option} requires {noun}");
            Err(ExitCode::from(2))
        }
    }
}

pub(crate) struct InspectArguments {
    pub sequence: whykey::key::KeySequence,
    pub target_pid: Option<u32>,
    pub focused: bool,
    pub instance: Option<String>,
}

pub(crate) fn parse_inspect_arguments(
    arguments: Vec<String>,
) -> Result<InspectArguments, ExitCode> {
    let mut combinations = Vec::new();
    let mut target_pid = None;
    let mut focused = false;
    let mut instance = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--pid" => {
                let value = take_value(&mut arguments, "--pid", "a positive process id")?;
                let pid = match value.parse::<u32>() {
                    Ok(pid) if pid > 0 => pid,
                    _ => {
                        eprintln!("error: --pid must be a positive process id");
                        return Err(ExitCode::from(2));
                    }
                };
                target_pid = Some(pid);
            }
            "--focused" => focused = true,
            "--instance" => {
                let value = take_value(&mut arguments, "--instance", "a Hyprland instance id")?;
                if value.trim().is_empty() {
                    eprintln!("error: --instance requires a non-empty Hyprland instance id");
                    return Err(ExitCode::from(2));
                }
                instance = Some(value);
            }
            _ if argument.starts_with('-') => {
                eprintln!("error: unknown inspect option '{argument}'");
                return Err(ExitCode::from(2));
            }
            _ => combinations.push(argument),
        }
    }

    if combinations.is_empty() {
        eprintln!(
            "error: usage is `whykey inspect <combination-or-sequence>`\n\n{}",
            help::help_text()
        );
        return Err(ExitCode::from(2));
    }
    if focused && target_pid.is_some() {
        eprintln!("error: --pid and --focused cannot be used together");
        return Err(ExitCode::from(2));
    }
    let input = combinations.join(" ");
    let sequence = match input.parse() {
        Ok(sequence) => sequence,
        Err(error) => {
            eprintln!("error: {error}\n\nTry 'whykey --help'.");
            return Err(ExitCode::from(2));
        }
    };
    Ok(InspectArguments {
        sequence,
        target_pid,
        focused,
        instance,
    })
}

/// Dispatch one already-separated command through the existing command
/// runners. This owns command selection while `main` remains responsible only
/// for global flags, help, and version handling.
pub(crate) fn dispatch(
    argument: String,
    raw_arguments: Vec<String>,
    json: bool,
    ndjson: bool,
    verbose: bool,
    color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let mut arguments = raw_arguments.into_iter();
    if ndjson && argument != "listen" {
        eprintln!("error: --ndjson is only supported by `whykey listen`");
        return ExitCode::from(2);
    }
    if argument == "inspect" {
        return crate::run_inspect(arguments.collect(), json, verbose, color, schema_version);
    }
    if argument == "listen" {
        let mut repeat = false;
        let mut terminal = false;
        let mut evdev = false;
        let mut device = None;
        let mut timeout = None;
        let mut count = None;
        let mut events_all = false;
        let mut output = None;
        let mut capture_policy = whykey::listen::CapturePolicy::default();
        let mut explicit_policy = None;
        let mut explicit_suppress = false;
        let mut dry_run = false;
        let mut explain_capture = false;
        while let Some(option) = arguments.next() {
            match option.as_str() {
                "--repeat" | "-r" => repeat = true,
                "--suppress" => {
                    if explicit_policy == Some("pass-through") {
                        eprintln!("error: --suppress and --pass-through cannot be used together");
                        return ExitCode::from(2);
                    }
                    capture_policy = whykey::listen::CapturePolicy::Suppress;
                    explicit_policy = Some("suppress");
                    explicit_suppress = true;
                }
                "--no-suppress" | "--pass-through" => {
                    if explicit_policy == Some("suppress") {
                        eprintln!("error: --suppress and --pass-through cannot be used together");
                        return ExitCode::from(2);
                    }
                    capture_policy = whykey::listen::CapturePolicy::PassThrough;
                    explicit_policy = Some("pass-through");
                }
                "--terminal" | "-t" => terminal = true,
                "--evdev" | "-e" => evdev = true,
                "--dry-run" => dry_run = true,
                "--explain-capture" => explain_capture = true,
                "-h" | "--help" => {
                    println!(
                        "Usage: whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--suppress|--no-suppress|--pass-through] [--dry-run] [--explain-capture] [--terminal] [--evdev] [--device PATH] [--verbose] [--color MODE] [--json] [--ndjson] [--output PATH]\n\nCapture one key and explain its path. Native capture observes without suppression by default; use --suppress for explicit temporary compositor suppression. It falls back to terminal capture when native capture is unavailable. --no-suppress captures without suppressing normal actions and --pass-through is its compatibility alias. --dry-run selects a backend without arming it; --explain-capture prints detection evidence.\nEsc or Ctrl+C exits; --repeat captures another deliberate key after each report. --timeout is a wall-clock deadline; --count stops after N reports.\nUse --events all with native compositor or evdev capture to include modifier-only and release events. --verbose shows every route layer and technical evidence. --color=auto respects NO_COLOR, non-TTY output, and TERM=dumb. --json emits full analysis; --ndjson streams one compact record per event; --output writes reports to a file."
                    );
                    return ExitCode::SUCCESS;
                }
                "--device" => {
                    let path = match take_value(&mut arguments, "--device", "a path") {
                        Ok(path) => path,
                        Err(code) => return code,
                    };
                    device = Some(path.into());
                    evdev = true;
                }
                "--timeout" => {
                    let value = match take_value(&mut arguments, "--timeout", "seconds") {
                        Ok(value) => value,
                        Err(code) => return code,
                    };
                    let seconds = match value.parse::<f64>() {
                        Ok(seconds) if seconds.is_finite() && seconds > 0.0 => seconds,
                        _ => {
                            eprintln!("error: --timeout must be a positive number of seconds");
                            return ExitCode::from(2);
                        }
                    };
                    let duration = match Duration::try_from_secs_f64(seconds) {
                        Ok(duration) if !duration.is_zero() => duration,
                        _ => {
                            eprintln!("error: --timeout is outside the supported duration range");
                            return ExitCode::from(2);
                        }
                    };
                    timeout = Some(duration);
                }
                "--count" => {
                    let value = match take_value(&mut arguments, "--count", "a positive integer") {
                        Ok(value) => value,
                        Err(code) => return code,
                    };
                    let parsed = match value.parse::<usize>() {
                        Ok(count) if count > 0 => count,
                        _ => {
                            eprintln!("error: --count must be a positive integer");
                            return ExitCode::from(2);
                        }
                    };
                    count = Some(parsed);
                }
                "--events" => {
                    let value = match take_value(&mut arguments, "--events", "'all'") {
                        Ok(value) => value,
                        Err(code) => return code,
                    };
                    if value != "all" {
                        eprintln!("error: unsupported event mode '{value}', expected 'all'");
                        return ExitCode::from(2);
                    }
                    events_all = true;
                }
                "--output" => {
                    let path = match take_value(&mut arguments, "--output", "a path") {
                        Ok(path) => path,
                        Err(code) => return code,
                    };
                    if path.is_empty() {
                        eprintln!("error: --output requires a non-empty path");
                        return ExitCode::from(2);
                    }
                    output = Some(path.into());
                }
                _ => {
                    eprintln!("error: unknown listen option '{option}'");
                    return ExitCode::from(2);
                }
            }
        }
        if output.is_some() && !json {
            eprintln!("error: --output requires --json or --ndjson");
            return ExitCode::from(2);
        }
        if terminal && (evdev || device.is_some()) {
            eprintln!("error: --terminal and --evdev cannot be used together");
            return ExitCode::from(2);
        }
        if events_all && terminal {
            eprintln!("error: --events all is not supported with --terminal");
            return ExitCode::from(2);
        }
        return crate::run_listen(whykey::listen::Options {
            repeat,
            json,
            ndjson,
            terminal,
            evdev,
            device,
            timeout,
            count,
            events_all,
            output,
            verbose,
            color,
            schema_version,
            capture_policy,
            explicit_suppress,
            dry_run,
            explain_capture,
        });
    }
    if argument == "doctor" {
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey doctor [--color MODE] [--json]`");
            return ExitCode::from(2);
        }
        return crate::run_doctor(json, color, schema_version);
    }
    if argument == "capabilities" {
        let mut all = false;
        for option in arguments.by_ref() {
            if option == "--all" {
                all = true;
            } else {
                eprintln!("error: usage is `whykey capabilities [--all] [--color MODE] [--json]`");
                return ExitCode::from(2);
            }
        }
        return crate::run_capabilities(json, all, color, schema_version);
    }
    if argument == "bindings" || argument == "conflicts" {
        let run: fn(bool, u8, InventoryFilters, ColorChoice) -> ExitCode = if argument == "bindings"
        {
            crate::run_bindings
        } else {
            crate::run_conflicts
        };
        let usage = if argument == "bindings" {
            "whykey bindings [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--color MODE] [--json] [--schema-version 2]"
        } else {
            "whykey conflicts [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--color MODE] [--json] [--schema-version 2]"
        };
        let filters = match parse_inventory_filters(arguments.collect(), usage) {
            Ok(filters) => filters,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(2);
            }
        };
        return run(json, schema_version, filters, color);
    }
    if argument == "extension" {
        let Some(program) = arguments.next() else {
            eprintln!(
                "error: usage is `whykey extension <program> <combination> [--color MODE] [--json]`"
            );
            return ExitCode::from(2);
        };
        let combinations: Vec<_> = arguments.collect();
        if combinations.is_empty() {
            eprintln!(
                "error: usage is `whykey extension <program> <combination> [--color MODE] [--json]`"
            );
            return ExitCode::from(2);
        }
        return crate::run_extension(program, combinations.join(" "), json, color, schema_version);
    }
    if argument == "replay" {
        let Some(path) = arguments.next() else {
            eprintln!("error: usage is `whykey replay <file> [--verbose] [--color MODE] [--json]`");
            return ExitCode::from(2);
        };
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey replay <file> [--verbose] [--color MODE] [--json]`");
            return ExitCode::from(2);
        }
        return crate::run_replay(path, json, verbose, color, schema_version);
    }
    if argument == "snapshot" {
        return crate::run_snapshot(arguments.collect());
    }
    if argument == "diff" {
        let paths: Vec<_> = arguments.collect();
        if paths.len() != 2 {
            eprintln!("error: usage is `whykey diff <before> <after> [--color MODE] [--json]`");
            return ExitCode::from(2);
        }
        return crate::run_diff(paths, json, color, schema_version);
    }
    if argument == "shell-init" || argument == "completions" {
        let print: fn(&str) -> String = if argument == "shell-init" {
            crate::shell_init
        } else {
            crate::completions
        };
        match arguments.next().as_deref() {
            Some(shell @ ("bash" | "zsh" | "fish")) if arguments.next().is_none() => {
                println!("{}", print(shell));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("error: usage is `whykey {argument} <bash|zsh|fish>`");
                return ExitCode::from(2);
            }
        }
    }
    if arguments.next().is_some() {
        eprintln!(
            "error: expected one key combination\n\n{}",
            help::help_text()
        );
        return ExitCode::from(2);
    }

    let key: whykey::key::KeyCombo = match argument.parse() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}\n\nTry 'whykey --help'.");
            return ExitCode::from(2);
        }
    };
    let results =
        whykey::command::with_deadline(whykey::command::configured_diagnostic_timeout(), || {
            whykey::layers::inspect_default_chain(&key)
        });
    let unavailable = exit_for_inspection(&results, false);
    if json {
        print!(
            "{}",
            whykey::report::render_json(&key, &results, None, schema_version)
        );
    } else {
        print!(
            "{}",
            whykey::report::render_with_options(
                &key,
                &results,
                whykey::style::RenderOptions::cli(color, verbose),
            )
        );
    }
    unavailable
}

pub(crate) fn inspection_failed(layers: &[LayerResult], has_explicit_target: bool) -> bool {
    let target_unavailable = layers.iter().any(|layer| {
        layer.id == LayerId::Application && layer.status() == LayerStatus::Unavailable
    });
    if has_explicit_target && target_unavailable {
        return true;
    }
    if layers
        .iter()
        .any(|layer| layer.id == LayerId::Diagnostic && layer.status() == LayerStatus::Unavailable)
    {
        return true;
    }
    let has_handled = layers.iter().any(|layer| {
        layer.status() == LayerStatus::Handled
            || layer.propagation() == Propagation::Stops
            || layer.propagation() == Propagation::Redirected
    });
    if has_handled {
        return false;
    }
    !layers.is_empty()
        && layers
            .iter()
            .all(|layer| matches!(layer.outcome, Outcome::Unavailable))
}

pub(crate) fn exit_for_inspection(layers: &[LayerResult], has_explicit_target: bool) -> ExitCode {
    if inspection_failed(layers, has_explicit_target) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

pub(crate) mod doctor_support {
    use std::env;
    use std::process::Command;

    use whykey::command;

    pub(crate) struct DoctorState {
        pub(crate) tty: bool,
        pub(crate) compositor_context: bool,
        pub(crate) ghostty: bool,
        pub(crate) generic_terminal: bool,
        pub(crate) xkb: bool,
        pub(crate) evdev: bool,
        pub(crate) shell_snapshot: bool,
        pub(crate) ssh: bool,
        pub(crate) remapper: bool,
        pub(crate) ime: bool,
    }

    pub(crate) fn doctor_next_steps(state: &DoctorState) -> Vec<&'static str> {
        let mut steps = Vec::new();
        if !state.tty {
            steps.push("run whykey from a controlling terminal");
        }
        if !state.compositor_context && !state.ssh {
            steps.push("run whykey inside the desktop session to identify the compositor");
        }
        if !state.ghostty && !state.generic_terminal {
            steps.push("install or configure a supported terminal adapter");
        }
        if !state.xkb {
            steps.push("install xkbcommon-tools for layout-aware keycode translation");
        }
        if !state.evdev {
            steps.push("grant read access to /dev/input/event* before using --evdev");
        }
        if !state.shell_snapshot {
            steps.push("load shell-init in the current shell for live shell bindings");
        }
        if state.remapper {
            steps.push(
                "compare remapper evidence with the physical capture; timing and routing remain conditional",
            );
        }
        if state.ime {
            steps
                .push("check application-side Compose/dead-key or preedit state when text differs");
        }
        steps
    }

    pub(crate) fn shell_snapshot_available(shell: &str) -> bool {
        match shell.rsplit('/').next().unwrap_or(shell) {
            "bash" => env::var_os("WHYKEY_READLINE_BINDINGS").is_some(),
            "zsh" => env::var_os("WHYKEY_ZLE_BINDINGS").is_some(),
            "fish" => env::var_os("WHYKEY_FISH_BINDINGS").is_some(),
            _ => false,
        }
    }

    pub(crate) fn command_succeeds(program: &str, arguments: &[&str]) -> bool {
        let mut command = Command::new(program);
        command.args(arguments);
        command::output(&mut command)
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    pub(crate) fn command_program_available(argv: &[String]) -> bool {
        let Some(program) = argv.first() else {
            return false;
        };
        let path = std::path::Path::new(program);
        if path.components().count() > 1 {
            return path.is_file();
        }
        env::var_os("PATH").is_some_and(|path_value| {
            env::split_paths(&path_value).any(|directory| directory.join(program).is_file())
        })
    }

    pub(crate) fn detected_terminal_program() -> Option<String> {
        env::var("TERM_PROGRAM")
            .ok()
            .or_else(|| env::var("LC_TERMINAL").ok())
            .or_else(|| {
                let term = env::var("TERM").ok()?;
                let normalized = term.to_ascii_lowercase();
                matches!(
                    normalized.as_str(),
                    "xterm-kitty" | "foot" | "foot-direct" | "alacritty" | "xterm-ghostty"
                )
                .then_some(term)
            })
    }

    pub(crate) fn detect_extension(name: &str) -> bool {
        if let Some(paths) = env::var_os("PATH") {
            for directory in env::split_paths(&paths) {
                if directory.join(name).is_file() {
                    return true;
                }
            }
        }
        std::path::Path::new("extensions").join(name).is_file()
    }

    pub(crate) fn detect_nvim_server() -> Option<String> {
        env::var("NVIM_LISTEN_ADDRESS")
            .ok()
            .or_else(|| env::var("NVIM").ok())
            .filter(|value| !value.is_empty())
            .or_else(scan_proc_nvim_server)
    }

    fn scan_proc_nvim_server() -> Option<String> {
        let entries = std::fs::read_dir("/proc").ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let pid_str = name.to_str()?;
            if !pid_str.chars().all(|character| character.is_ascii_digit()) {
                continue;
            }
            let cmdline_path = entry.path().join("cmdline");
            let Ok(bytes) = std::fs::read(cmdline_path) else {
                continue;
            };
            let cmd = String::from_utf8_lossy(&bytes);
            let first_arg = cmd.split('\0').next().unwrap_or("");
            if first_arg == "nvim" || first_arg.ends_with("/nvim") {
                if let Some(server) = whykey::layers::application::nvim_server_from_command(&cmd) {
                    return Some(server);
                }
            }
        }
        None
    }

    pub(crate) fn print_check_with_options(
        name: &str,
        ok: bool,
        hint: &str,
        options: whykey::style::RenderOptions,
    ) {
        let text = if ok {
            format!("OK  {name}")
        } else {
            format!("FAIL  {name} ({hint})")
        };
        for line in whykey::style::wrap_hanging(&text, options.width, "", "  ") {
            if ok {
                println!("{}", options.success(line));
            } else {
                println!("{}", options.failure(line));
            }
        }
    }
}

pub(crate) use doctor_support::{
    DoctorState, command_program_available, command_succeeds, detect_extension, detect_nvim_server,
    detected_terminal_program, doctor_next_steps, print_check_with_options,
    shell_snapshot_available,
};

pub(crate) mod help {
    use std::env;

    pub(crate) struct CommandSpec {
        pub name: &'static str,
        pub usage: &'static str,
        pub summary: &'static str,
        pub advanced: bool,
    }

    pub(crate) const COMMANDS: &[CommandSpec] = &[
        CommandSpec {
            name: "inspect",
            usage: "whykey inspect [--pid PID] [--focused] [--instance ID] [--verbose] [--color MODE] [--json] [--schema-version 2] <combination-or-sequence>",
            summary: "explain one combination or sequence",
            advanced: false,
        },
        CommandSpec {
            name: "listen",
            usage: "whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--suppress|--no-suppress|--pass-through] [--terminal] [--evdev] [--device PATH] [--verbose] [--color MODE] [--json] [--ndjson] [--output PATH] [--schema-version 2]",
            summary: "capture a key and explain its path (Hyprland and Sway; Sway is pass-through-only)",
            advanced: false,
        },
        CommandSpec {
            name: "doctor",
            usage: "whykey doctor [--color MODE] [--json] [--schema-version 2]",
            summary: "check session integrations",
            advanced: false,
        },
        CommandSpec {
            name: "capabilities",
            usage: "whykey capabilities [--all] [--color MODE] [--json] [--schema-version 2]",
            summary: "list applicable integrations (--all for the full inventory)",
            advanced: true,
        },
        CommandSpec {
            name: "bindings",
            usage: "whykey bindings [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--color MODE] [--json] [--schema-version 2]",
            summary: "enumerate effective bindings",
            advanced: true,
        },
        CommandSpec {
            name: "conflicts",
            usage: "whykey conflicts [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--color MODE] [--json] [--schema-version 2]",
            summary: "group duplicate actions",
            advanced: true,
        },
        CommandSpec {
            name: "extension",
            usage: "whykey extension <program> <combination> [--color MODE] [--json] [--schema-version 2]",
            summary: "ask one opt-in external diagnostic",
            advanced: true,
        },
        CommandSpec {
            name: "replay",
            usage: "whykey replay <file> [--verbose] [--color MODE] [--json]",
            summary: "re-render saved reports without injecting input",
            advanced: true,
        },
        CommandSpec {
            name: "snapshot",
            usage: "whykey snapshot <combination> [--output PATH]",
            summary: "save a static diagnostic snapshot without capturing input",
            advanced: true,
        },
        CommandSpec {
            name: "diff",
            usage: "whykey diff <before> <after> [--color MODE] [--json]",
            summary: "compare saved snapshots without querying the desktop",
            advanced: true,
        },
        CommandSpec {
            name: "shell-init",
            usage: "whykey shell-init <bash|zsh|fish>",
            summary: "print shell integration",
            advanced: true,
        },
        CommandSpec {
            name: "completions",
            usage: "whykey completions <bash|zsh|fish>",
            summary: "print shell completions",
            advanced: true,
        },
    ];

    pub(crate) const COMPLETION_SHELLS: &[&str] = &["bash", "zsh", "fish"];
    const COMPLETION_EXAMPLES: &[&str] = &["ctrl+left", "ctrl+z", "super+c"];
    const COMMON_COMPLETION_OPTIONS: &[&str] = &[
        "--json",
        "--json-v2",
        "--schema-version",
        "--verbose",
        "--color",
    ];
    const INSPECT_COMPLETION_OPTIONS: &[&str] = &["--pid", "--focused", "--instance", "--verbose"];
    const LISTEN_COMPLETION_OPTIONS: &[&str] = &[
        "--repeat",
        "--timeout",
        "--count",
        "--events",
        "--suppress",
        "--no-suppress",
        "--dry-run",
        "--explain-capture",
        "--terminal",
        "--evdev",
        "--device",
        "--ndjson",
        "--output",
        "--verbose",
    ];
    const SNAPSHOT_COMPLETION_OPTIONS: &[&str] = &["--output"];
    pub(crate) const INVENTORY_FILTER_OPTIONS: &[&str] =
        &["--key", "--action", "--source", "--device", "--submap"];

    /// Primary help leads with the common use; advanced commands follow.
    /// Every usage line comes from [`COMMANDS`], so help, the parser, and
    /// completions share one vocabulary.
    pub(crate) fn help_text() -> String {
        let mut output =
            String::from("whykey - explain where a key combination is handled\n\nUsage:\n");
        output.push_str("  whykey [--verbose] [--json] [--color auto|always|never] [--schema-version 2] <combination>\n");
        for command in COMMANDS.iter().filter(|command| !command.advanced) {
            output.push_str(&format!("  {}\n    {}\n", command.usage, command.summary));
        }
        output.push_str("\nAdvanced:\n");
        for command in COMMANDS.iter().filter(|command| command.advanced) {
            output.push_str(&format!("  {}\n    {}\n", command.usage, command.summary));
        }
        output.push_str(
            "\nExamples:\n  whykey ctrl+left\n  whykey ctrl+z\n  whykey super+c\n  whykey inspect ctrl+x ctrl+s\n\nHuman-readable reports lead with a diagnosis and keep the relevant evidence nearby. Add --verbose for the complete route, raw events, and configuration details. --color=auto is the default and respects NO_COLOR, non-TTY output, and TERM=dumb; --color=always and --color=never take precedence over those environment checks. JSON, NDJSON, shell-init, and completion output are always plain.\n\nwhykey does not edit configuration or execute shortcuts. Native compositor listen supports Hyprland suppression and Hyprland/Sway pass-through-only capture; temporary compositor state is restored when capture ends.",
        );
        output
    }

    pub(crate) fn shell_init(shell: &str) -> String {
        let binary = env::current_exe()
            .ok()
            .map(|path| shell_quote(&path.to_string_lossy()))
            .unwrap_or_else(|| "whykey".into());
        match shell {
            "bash" => format!(
                "whykey() {{\n  local whykey_readline_bindings whykey_readline_macros whykey_readline_shell_bindings whykey_readline_variables whykey_readline_map\n  whykey_readline_bindings=\"$(builtin bind -P 2>/dev/null)\"\n  for whykey_readline_map in emacs emacs-standard emacs-meta vi vi-move vi-command vi-insertion; do\n    whykey_readline_bindings=\"$whykey_readline_bindings\n# WHYKEY_KEYMAP $whykey_readline_map\n$(builtin bind -m \"$whykey_readline_map\" -P 2>/dev/null)\"\ndone\n  whykey_readline_macros=\"$(builtin bind -S 2>/dev/null)\"\n  whykey_readline_shell_bindings=\"$(builtin bind -X 2>/dev/null)\"\n  whykey_readline_variables=\"$(builtin bind -V 2>/dev/null)\"\n  WHYKEY_READLINE_BINDINGS=\"$whykey_readline_bindings\" WHYKEY_READLINE_MACROS=\"$whykey_readline_macros\" WHYKEY_READLINE_SHELL_BINDINGS=\"$whykey_readline_shell_bindings\" WHYKEY_READLINE_VARIABLES=\"$whykey_readline_variables\" command {binary} \"$@\"\n}}"
            ),
            "zsh" => format!(
                "whykey() {{\n  local whykey_zle_bindings\n  whykey_zle_bindings=\"$(bindkey -L 2>/dev/null)\"\n  WHYKEY_ZLE_BINDINGS=\"$whykey_zle_bindings\" command {binary} \"$@\"\n}}"
            ),
            "fish" => format!(
                "function whykey\n  set -lx WHYKEY_FISH_BINDINGS (string join \\n -- (bind --all 2>/dev/null))\n  set -lx WHYKEY_FISH_MODE $fish_bind_mode\n  command {binary} $argv\nend"
            ),
            _ => unreachable!("shell-init only accepts supported shells"),
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    pub(crate) fn completions(shell: &str) -> String {
        let commands = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>()
            .join(" ");
        let shells = COMPLETION_SHELLS.join(" ");
        let examples = COMPLETION_EXAMPLES.join(" ");
        let common_options = COMMON_COMPLETION_OPTIONS.join(" ");
        let inspect_options = INSPECT_COMPLETION_OPTIONS.join(" ");
        let listen_options = LISTEN_COMPLETION_OPTIONS.join(" ");
        let mut script = match shell {
            "bash" => r#"_whykey_complete() {
      local cur="${COMP_WORDS[COMP_CWORD]}"
      if (( COMP_CWORD == 1 )); then
        COMPREPLY=( $(compgen -W "__COMMANDS__ __EXAMPLES__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "listen" ]]; then
        COMPREPLY=( $(compgen -W "__LISTEN_OPTIONS__ __COMMON_OPTIONS__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "inspect" ]]; then
        COMPREPLY=( $(compgen -W "__INSPECT_OPTIONS__ __COMMON_OPTIONS__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "doctor" || "${COMP_WORDS[1]}" == "diff" ]]; then
        COMPREPLY=( $(compgen -W "__COMMON_OPTIONS__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "bindings" || "${COMP_WORDS[1]}" == "conflicts" ]]; then
        COMPREPLY=( $(compgen -W "__COMMON_OPTIONS__ __INVENTORY_FILTER_OPTIONS__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "snapshot" ]]; then
        COMPREPLY=( $(compgen -W "__SNAPSHOT_OPTIONS__" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "capabilities" ]]; then
        COMPREPLY=( $(compgen -W "__COMMON_OPTIONS__ --all" -- "$cur") )
      elif [[ "${COMP_WORDS[1]}" == "shell-init" || "${COMP_WORDS[1]}" == "completions" ]]; then
        COMPREPLY=( $(compgen -W "__SHELLS__" -- "$cur") )
      fi
    }
    complete -F _whykey_complete whykey
    "#,
            "zsh" => r#"#compdef whykey
    _whykey() {
      _arguments \
        '1:command:(__COMMANDS__)' \
        '2:argument:(__SHELLS__)' \
        '--json[emit JSON]' \
        '--json-v2[emit JSON schema v2]' \
        '--schema-version[select JSON schema version]:version:(1 2)' \
        '--verbose[show every route layer]' \
        '--color[set terminal colors]:mode:(auto always never)' \
        '--all[list the complete adapter inventory]' \
        '--key[filter by key]:text:' \
        '--action[filter by action]:text:' \
        '--source[filter by source]:text:' \
        '--submap[filter by submap]:text:' \
        '--pid[inspect process ancestry]:pid:' \
        '--focused[inspect focused window]' \
        '--instance[select a Hyprland instance]:instance:' \
        '--repeat[keep listening]' \
        '--timeout[stop after this many seconds]:seconds:' \
        '--count[stop after this many reports]:count:' \
        '--events[include modifier and release events]:mode:(all)' \
        '--suppress[temporarily suppress compositor shortcuts during capture]' \
        '--no-suppress[capture without suppressing compositor shortcuts]' \
        '--dry-run[select capture backend without arming it]' \
        '--explain-capture[show capture detection evidence]' \
        '--terminal[force terminal capture]' \
        '--evdev[capture Linux input events before the compositor]' \
        '--device[read one /dev/input/event device]:path:' \
        '--ndjson[emit one compact schema-v2 record per captured event]' \
        '--output[write captured JSON reports to a file]:path:' \
        '*:key combination:'
    }
    _whykey "$@"
    "#,
            "fish" => r#"complete -c whykey -f -n '__fish_use_subcommand' -a '__COMMANDS__'
    complete -c whykey -f -n '__fish_seen_subcommand_from shell-init completions' -a '__SHELLS__'
    complete -c whykey -l json -d 'emit JSON'
    complete -c whykey -l json-v2 -d 'emit JSON schema v2'
    complete -c whykey -l schema-version -r -a '1 2' -d 'select JSON schema version'
    complete -c whykey -l verbose -d 'show every route layer'
    complete -c whykey -l color -r -a 'auto always never' -d 'set terminal colors'
    complete -c whykey -n '__fish_seen_subcommand_from bindings conflicts' -l key -r -d 'filter by key'
    complete -c whykey -n '__fish_seen_subcommand_from bindings conflicts' -l action -r -d 'filter by action'
    complete -c whykey -n '__fish_seen_subcommand_from bindings conflicts' -l source -r -d 'filter by source'
    complete -c whykey -n '__fish_seen_subcommand_from bindings conflicts' -l submap -r -d 'filter by submap'
    complete -c whykey -n '__fish_seen_subcommand_from capabilities' -l all -d 'list the complete adapter inventory'
    complete -c whykey -l pid -r -d 'inspect the process ancestry rooted at this PID'
    complete -c whykey -l focused -d 'inspect the focused window process'
    complete -c whykey -l instance -r -d 'select a Hyprland instance explicitly'
    complete -c whykey -s r -l repeat -d 'keep listening'
    complete -c whykey -l timeout -r -d 'stop after this many seconds'
    complete -c whykey -l count -r -d 'stop after this many reports'
    complete -c whykey -l events -r -a 'all' -d 'include modifier and release events'
    complete -c whykey -l suppress -d 'temporarily suppress compositor shortcuts during capture'
    complete -c whykey -l no-suppress -d 'capture without suppressing compositor shortcuts'
    complete -c whykey -l dry-run -d 'select capture backend without arming it'
    complete -c whykey -l explain-capture -d 'show capture detection evidence'
    complete -c whykey -s t -l terminal -d 'force terminal capture'
    complete -c whykey -s e -l evdev -d 'capture Linux input events before the compositor'
    complete -c whykey -l device -r -d 'read one /dev/input/event device'
    complete -c whykey -n '__fish_seen_subcommand_from listen' -l ndjson -d 'emit one compact schema-v2 JSON record per captured event'
    complete -c whykey -n '__fish_seen_subcommand_from listen' -l output -r -d 'write captured JSON reports to a file'
    complete -c whykey -n '__fish_seen_subcommand_from snapshot' -l output -r -d 'write a static diagnostic snapshot to a file'
    "#,
            _ => unreachable!("completions only accepts supported shells"),
        }
        .to_owned();
        for (placeholder, value) in [
            ("__COMMANDS__", commands),
            ("__SHELLS__", shells),
            ("__EXAMPLES__", examples),
            ("__COMMON_OPTIONS__", common_options),
            ("__INSPECT_OPTIONS__", inspect_options),
            ("__LISTEN_OPTIONS__", listen_options),
            (
                "__SNAPSHOT_OPTIONS__",
                SNAPSHOT_COMPLETION_OPTIONS.join(" "),
            ),
            (
                "__INVENTORY_FILTER_OPTIONS__",
                INVENTORY_FILTER_OPTIONS.join(" "),
            ),
        ] {
            script = script.replace(placeholder, &value);
        }
        script
    }
}

#[cfg(test)]
pub(crate) use help::{COMMANDS, COMPLETION_SHELLS};
pub(crate) use help::{completions, help_text, shell_init};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_global_flags_without_reordering_command_arguments() {
        let parsed = parse_global_arguments(vec![
            "--json-v2".into(),
            "inspect".into(),
            "ctrl+x".into(),
            "--verbose".into(),
        ])
        .unwrap();
        assert_eq!(parsed.arguments, vec!["inspect", "ctrl+x"]);
        assert!(parsed.json);
        assert!(parsed.verbose);
        assert_eq!(parsed.schema_version, 2);
    }

    #[test]
    fn rejects_invalid_schema_versions_at_the_parser_boundary() {
        assert_eq!(
            parse_global_arguments(vec!["--schema-version".into()]).unwrap_err(),
            "--schema-version requires 1 or 2"
        );
        assert_eq!(
            parse_global_arguments(vec!["--schema-version".into(), "3".into()]).unwrap_err(),
            "--schema-version must be 1 or 2"
        );
    }

    #[test]
    fn parses_explicit_color_modes_without_reordering_arguments() {
        let parsed = parse_global_arguments(vec![
            "--color=always".into(),
            "inspect".into(),
            "ctrl+z".into(),
            "--color".into(),
            "never".into(),
        ])
        .unwrap();
        assert_eq!(parsed.arguments, vec!["inspect", "ctrl+z"]);
        assert_eq!(parsed.color, ColorChoice::Never);
    }

    #[test]
    fn rejects_invalid_color_modes_at_the_parser_boundary() {
        assert!(
            parse_global_arguments(vec!["--color=rainbow".into()])
                .unwrap_err()
                .contains("expected auto, always, or never")
        );
    }
}
