use std::ffi::{OsStr, OsString};
use std::process::ExitCode;
use std::time::Duration;

use whykey::layers::{LayerId, LayerResult, LayerStatus, Outcome, Propagation};
use whykey::schema;
use whykey::style::ColorChoice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlobalArguments {
    pub arguments: Vec<OsString>,
    pub json: bool,
    pub ndjson: bool,
    pub verbose: bool,
    pub color: ColorChoice,
    pub schema_version: u8,
}

/// Remove global presentation flags while preserving command-local arguments.
/// Validation is centralized here so every dispatch path sees the same schema
/// and output-mode rules. Arguments stay `OsString` so file paths that are not
/// UTF-8 (legal on Linux) reach the commands that open them unchanged.
pub(crate) fn parse_global_arguments(raw: Vec<OsString>) -> Result<GlobalArguments, String> {
    fn flag(argument: &OsStr) -> &str {
        argument.to_str().unwrap_or("")
    }
    let json = raw
        .iter()
        .any(|argument| matches!(flag(argument), "--json" | "--json-v2" | "--ndjson"));
    let ndjson = raw.iter().any(|argument| argument == "--ndjson");
    let verbose = raw
        .iter()
        .any(|argument| matches!(flag(argument), "--verbose" | "-v"));
    let mut color = ColorChoice::Auto;
    let mut schema_version = None;
    let mut arguments = Vec::with_capacity(raw.len());
    let mut raw = raw.into_iter();
    while let Some(argument) = raw.next() {
        match flag(&argument) {
            "--json" | "--ndjson" | "--verbose" | "-v" => {}
            "--color" => {
                color = raw
                    .next()
                    .as_ref()
                    .and_then(|value| value.to_str())
                    .ok_or("--color requires auto, always, or never")?
                    .parse()?;
            }
            option if option.starts_with("--color=") => {
                color = option["--color=".len()..].parse()?;
            }
            "--json-v2" => select_schema_version(&mut schema_version, 2, "--json-v2")?,
            "--schema-version" => {
                let Some(value) = raw.next() else {
                    return Err("--schema-version requires 1 or 2".into());
                };
                let (version, flag) = match value.to_str().and_then(|value| value.parse().ok()) {
                    Some(1) => (1, "--schema-version 1"),
                    Some(2) => (2, "--schema-version 2"),
                    _ => return Err("--schema-version must be 1 or 2".into()),
                };
                select_schema_version(&mut schema_version, version, flag)?;
            }
            _ => arguments.push(argument),
        }
    }
    Ok(GlobalArguments {
        arguments,
        json,
        ndjson,
        verbose,
        color,
        schema_version: schema_version.map_or(schema::DEFAULT_VERSION, |(version, _)| version),
    })
}

/// Record one schema selection. Repeating the same version is harmless, but
/// two different versions are a contradiction, not a last-wins override.
fn select_schema_version(
    selected: &mut Option<(u8, &'static str)>,
    version: u8,
    flag: &'static str,
) -> Result<(), String> {
    match *selected {
        Some((previous, previous_flag)) if previous != version => Err(format!(
            "{previous_flag} and {flag} select different JSON schema versions"
        )),
        Some(_) => Ok(()),
        None => {
            *selected = Some((version, flag));
            Ok(())
        }
    }
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
pub(crate) fn take_value<T>(
    arguments: &mut impl Iterator<Item = T>,
    option: &str,
    noun: &str,
) -> Result<T, ExitCode> {
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

/// A command's parsed arguments, or its request for help text.
pub(crate) enum Parsed<T> {
    Help,
    Run(T),
}

const LISTEN_HELP: &str = "Capture one key and explain its path. By default the first usable backend wins: native compositor capture (Hyprland, Sway), then a focused probe window when a Wayland or X11 display is usable (press the shortcut there; it reports whether the desktop delivered the key or consumed it), then terminal capture. Native capture observes without suppression by default; use --suppress for explicit temporary compositor suppression, which fails instead of falling back; --suppress cannot be combined with --terminal, --evdev, --device, or --window. --no-suppress captures without suppressing normal actions and --pass-through is its compatibility alias. --window forces the probe window and --terminal forces terminal capture. --dry-run prints the capture chain and the backend it selects without arming it; --explain-capture prints detection evidence and the chain.\nEsc or Ctrl+C exits; --repeat captures another deliberate key after each report. --timeout is a wall-clock deadline; --count stops after N reports.\nUse --events all with native compositor, evdev, or window capture to include modifier-only and release events. --verbose shows every route layer and technical evidence. --color=auto respects NO_COLOR, non-TTY output, and TERM=dumb. --json emits full analysis; --ndjson streams one compact record per event; --output writes reports to a file.";

pub(crate) fn parse_listen_arguments(
    arguments: Vec<String>,
    json: bool,
    ndjson: bool,
    verbose: bool,
    color: ColorChoice,
    schema_version: u8,
) -> Result<Parsed<whykey::listen::Options>, ExitCode> {
    let mut arguments = arguments.into_iter();
    let mut repeat = false;
    let mut terminal = false;
    let mut evdev = false;
    let mut window = false;
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
                    eprintln!("error: --suppress and --no-suppress cannot be used together");
                    return Err(ExitCode::from(2));
                }
                capture_policy = whykey::listen::CapturePolicy::Suppress;
                explicit_policy = Some("suppress");
                explicit_suppress = true;
            }
            "--no-suppress" | "--pass-through" => {
                if explicit_policy == Some("suppress") {
                    eprintln!("error: --suppress and --no-suppress cannot be used together");
                    return Err(ExitCode::from(2));
                }
                capture_policy = whykey::listen::CapturePolicy::PassThrough;
                explicit_policy = Some("pass-through");
            }
            "--terminal" | "-t" => terminal = true,
            "--evdev" | "-e" => evdev = true,
            "--window" | "-w" => window = true,
            "--dry-run" => dry_run = true,
            "--explain-capture" => explain_capture = true,
            "-h" | "--help" => return Ok(Parsed::Help),
            "--device" => {
                device = Some(take_value(&mut arguments, "--device", "a path")?.into());
                evdev = true;
            }
            "--timeout" => {
                let value = take_value(&mut arguments, "--timeout", "seconds")?;
                let seconds = match value.parse::<f64>() {
                    Ok(seconds) if seconds.is_finite() && seconds > 0.0 => seconds,
                    _ => {
                        eprintln!("error: --timeout must be a positive number of seconds");
                        return Err(ExitCode::from(2));
                    }
                };
                let duration = match Duration::try_from_secs_f64(seconds) {
                    Ok(duration) if !duration.is_zero() => duration,
                    _ => {
                        eprintln!("error: --timeout is outside the supported duration range");
                        return Err(ExitCode::from(2));
                    }
                };
                timeout = Some(duration);
            }
            "--count" => {
                let value = take_value(&mut arguments, "--count", "a positive integer")?;
                let parsed = match value.parse::<usize>() {
                    Ok(count) if count > 0 => count,
                    _ => {
                        eprintln!("error: --count must be a positive integer");
                        return Err(ExitCode::from(2));
                    }
                };
                count = Some(parsed);
            }
            "--events" => {
                let value = take_value(&mut arguments, "--events", "'all'")?;
                if value != "all" {
                    eprintln!("error: unsupported event mode '{value}', expected 'all'");
                    return Err(ExitCode::from(2));
                }
                events_all = true;
            }
            "--output" => {
                let path = take_value(&mut arguments, "--output", "a path")?;
                if path.is_empty() {
                    eprintln!("error: --output requires a non-empty path");
                    return Err(ExitCode::from(2));
                }
                output = Some(path.into());
            }
            _ => {
                eprintln!("error: unknown listen option '{option}'");
                return Err(ExitCode::from(2));
            }
        }
    }
    if output.is_some() && !json {
        eprintln!("error: --output requires --json or --ndjson");
        return Err(ExitCode::from(2));
    }
    if terminal && (evdev || device.is_some()) {
        eprintln!("error: --terminal and --evdev cannot be used together");
        return Err(ExitCode::from(2));
    }
    if window && (terminal || evdev || device.is_some()) {
        eprintln!("error: --window cannot be combined with --terminal, --evdev, or --device");
        return Err(ExitCode::from(2));
    }
    if let Some(conflict) = whykey::listen::suppress_conflict(
        capture_policy,
        terminal,
        evdev || device.is_some(),
        window,
    ) {
        eprintln!("error: {conflict}");
        return Err(ExitCode::from(2));
    }
    if events_all && terminal {
        eprintln!("error: --events all is not supported with --terminal");
        return Err(ExitCode::from(2));
    }
    Ok(Parsed::Run(whykey::listen::Options {
        repeat,
        json,
        ndjson,
        terminal,
        evdev,
        window,
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
    }))
}

/// Returns whether `--all` was given.
pub(crate) fn parse_capabilities_arguments(arguments: Vec<String>) -> Result<bool, ExitCode> {
    let mut all = false;
    for option in arguments {
        if option != "--all" {
            eprintln!("error: usage is `{}`", usage("capabilities"));
            return Err(ExitCode::from(2));
        }
        all = true;
    }
    Ok(all)
}

pub(crate) struct SnapshotRequest {
    pub combination: String,
    pub output_path: Option<OsString>,
}

/// Require a UTF-8 argument. Keys, options and values other than file paths
/// are text; a non-UTF-8 one is a usage error, never a panic.
pub(crate) fn utf8_argument(argument: OsString) -> Result<String, ExitCode> {
    argument.into_string().map_err(|argument| {
        eprintln!(
            "error: argument {} is not valid UTF-8",
            whykey::key::abbreviate(&argument.to_string_lossy())
        );
        ExitCode::from(2)
    })
}

pub(crate) fn utf8_arguments(
    arguments: impl Iterator<Item = OsString>,
) -> Result<Vec<String>, ExitCode> {
    arguments.map(utf8_argument).collect()
}

pub(crate) fn parse_snapshot_arguments(
    arguments: Vec<OsString>,
) -> Result<Parsed<SnapshotRequest>, ExitCode> {
    let mut arguments = arguments.into_iter();
    let Some(combination) = arguments.next() else {
        eprintln!("error: usage is `{}`", usage("snapshot"));
        return Err(ExitCode::from(2));
    };
    let combination = utf8_argument(combination)?;
    if combination == "-h" || combination == "--help" {
        return Ok(Parsed::Help);
    }
    let mut output_path = None;
    while let Some(argument) = arguments.next() {
        if argument != "--output" {
            eprintln!("error: usage is `{}`", usage("snapshot"));
            return Err(ExitCode::from(2));
        }
        let path = take_value(&mut arguments, "--output", "a path")?;
        if path.is_empty() {
            eprintln!("error: --output requires a non-empty path");
            return Err(ExitCode::from(2));
        }
        output_path = Some(path);
    }
    Ok(Parsed::Run(SnapshotRequest {
        combination,
        output_path,
    }))
}

/// Dispatch one already-separated command through the existing command
/// runners. This owns command selection while `main` remains responsible only
/// for global flags, help, and version handling. File-path arguments (replay,
/// diff, snapshot `--output`, extension program) stay `OsString`; every other
/// argument must be UTF-8.
pub(crate) fn dispatch(
    argument: String,
    raw_arguments: Vec<OsString>,
    json: bool,
    ndjson: bool,
    verbose: bool,
    color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    if ndjson && argument != "listen" {
        eprintln!("error: --ndjson is only supported by `whykey listen`");
        return ExitCode::from(2);
    }
    if argument == "replay" {
        let mut arguments = raw_arguments.into_iter();
        let (Some(path), None) = (arguments.next(), arguments.next()) else {
            eprintln!("error: usage is `{}`", usage("replay"));
            return ExitCode::from(2);
        };
        return crate::run_replay(path.into(), json, verbose, color, schema_version);
    }
    if argument == "snapshot" {
        return match parse_snapshot_arguments(raw_arguments) {
            Ok(Parsed::Help) => {
                println!("Usage: {}", usage("snapshot"));
                println!("\nSave a static diagnostic snapshot. No key is captured or injected.");
                ExitCode::SUCCESS
            }
            Ok(Parsed::Run(request)) => crate::run_snapshot(request),
            Err(code) => code,
        };
    }
    if argument == "diff" {
        if raw_arguments.len() != 2 {
            eprintln!("error: usage is `{}`", usage("diff"));
            return ExitCode::from(2);
        }
        let paths = raw_arguments.into_iter().map(Into::into).collect();
        return crate::run_diff(paths, json, color, schema_version);
    }
    let mut raw_arguments = raw_arguments.into_iter();
    let extension_program = if argument == "extension" {
        let Some(program) = raw_arguments.next() else {
            eprintln!("error: usage is `{}`", usage("extension"));
            return ExitCode::from(2);
        };
        Some(program)
    } else {
        None
    };
    let mut arguments = match utf8_arguments(raw_arguments) {
        Ok(arguments) => arguments.into_iter(),
        Err(code) => return code,
    };
    if let Some(program) = extension_program {
        let combinations: Vec<_> = arguments.collect();
        if combinations.is_empty() {
            eprintln!("error: usage is `{}`", usage("extension"));
            return ExitCode::from(2);
        }
        return crate::run_extension(
            &program,
            combinations.join(" "),
            json,
            color,
            schema_version,
        );
    }
    if argument == "inspect" {
        return crate::run_inspect(arguments.collect(), json, verbose, color, schema_version);
    }
    if argument == "listen" {
        return match parse_listen_arguments(
            arguments.collect(),
            json,
            ndjson,
            verbose,
            color,
            schema_version,
        ) {
            Ok(Parsed::Help) => {
                println!("Usage: {}\n\n{LISTEN_HELP}", usage("listen"));
                ExitCode::SUCCESS
            }
            Ok(Parsed::Run(options)) => crate::run_listen(options),
            Err(code) => code,
        };
    }
    if argument == "doctor" {
        if arguments.next().is_some() {
            eprintln!("error: usage is `{}`", usage("doctor"));
            return ExitCode::from(2);
        }
        return crate::run_doctor(json, color, schema_version);
    }
    if argument == "capabilities" {
        return match parse_capabilities_arguments(arguments.collect()) {
            Ok(all) => crate::run_capabilities(json, all, color, schema_version),
            Err(code) => code,
        };
    }
    if argument == "bindings" || argument == "conflicts" {
        let run: fn(bool, u8, InventoryFilters, ColorChoice) -> ExitCode = if argument == "bindings"
        {
            crate::run_bindings
        } else {
            crate::run_conflicts
        };
        let filters = match parse_inventory_filters(arguments.collect(), &usage(&argument)) {
            Ok(filters) => filters,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(2);
            }
        };
        return run(json, schema_version, filters, color);
    }
    if argument == "shell-init" || argument == "completions" {
        let print: fn(&str) -> String = if argument == "shell-init" {
            crate::shell_init
        } else {
            crate::completions
        };
        match (arguments.next(), arguments.next()) {
            (Some(shell), None) if COMPLETION_SHELLS.contains(&shell.as_str()) => {
                println!("{}", print(&shell));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("error: usage is `{}`", usage(&argument));
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

    let key = match whykey::key::KeyCombo::parse_query(&argument) {
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
        pub(crate) evdev: whykey::doctor::EvdevScan,
        /// `whykey::doctor::evdev_acl_command` for the detected keyboards.
        pub(crate) evdev_acl: Option<String>,
        pub(crate) shell_snapshot: bool,
        pub(crate) ssh: bool,
        pub(crate) remapper: bool,
        pub(crate) ime: bool,
    }

    pub(crate) fn doctor_next_steps(state: &DoctorState) -> Vec<String> {
        let mut steps: Vec<String> = Vec::new();
        if !state.tty {
            steps.push("run whykey from a controlling terminal".into());
        }
        if !state.compositor_context && !state.ssh {
            steps.push("run whykey inside the desktop session to identify the compositor".into());
        }
        if !state.ghostty && !state.generic_terminal {
            steps.push("install or configure a supported terminal adapter".into());
        }
        if !state.xkb {
            steps.push("install xkbcommon-tools for layout-aware keycode translation".into());
        }
        if let Some(step) = state
            .evdev_acl
            .clone()
            .or_else(|| state.evdev.next_step().map(str::to_owned))
        {
            steps.push(step);
        }
        if !state.shell_snapshot {
            steps.push("load shell-init in the current shell for live shell bindings".into());
        }
        if state.remapper {
            steps.push(
                "compare remapper evidence with the physical capture; timing and routing remain conditional"
                    .into(),
            );
        }
        if state.ime {
            steps.push(
                "check application-side Compose/dead-key or preedit state when text differs".into(),
            );
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
        let set = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        set("TERM_PROGRAM")
            .or_else(|| set("LC_TERMINAL"))
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
    use std::fmt::Write as _;

    /// How an option's value is written and completed.
    #[derive(Clone, Copy)]
    pub(crate) enum OptionValue {
        /// A switch without a value.
        None,
        /// Free text, shown with this placeholder.
        Text(&'static str),
        /// A filesystem path.
        Path,
        /// One of a fixed set of words.
        Choices(&'static [&'static str]),
    }

    /// One command-line option. Usage lines, all three shell completion
    /// scripts, and the parser tests read these records, so an option cannot
    /// be added to one of them without the others following.
    pub(crate) struct OptionSpec {
        pub long: &'static str,
        pub short: Option<&'static str>,
        pub value: OptionValue,
        pub help: &'static str,
        /// Rendered in usage as an alternative of the preceding option.
        pub alternative: bool,
        /// Compatibility alias: accepted and shown in usage, never completed.
        pub alias: bool,
    }

    impl OptionSpec {
        const fn switch(long: &'static str, help: &'static str) -> Self {
            Self::valued(long, OptionValue::None, help)
        }

        const fn valued(long: &'static str, value: OptionValue, help: &'static str) -> Self {
            Self {
                long,
                short: None,
                value,
                help,
                alternative: false,
                alias: false,
            }
        }

        const fn short(mut self, short: &'static str) -> Self {
            self.short = Some(short);
            self
        }

        const fn alternative(mut self) -> Self {
            self.alternative = true;
            self
        }

        const fn alias(mut self) -> Self {
            self.alias = true;
            self
        }
    }

    /// What follows the command name besides options.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Operand {
        None,
        Text,
        Files,
        Shells,
    }

    pub(crate) struct CommandSpec {
        pub name: &'static str,
        /// Operand usage, for example `<combination-or-sequence>`.
        pub operands: &'static str,
        pub operand: Operand,
        pub options: &'static [&'static [OptionSpec]],
        pub summary: &'static str,
        pub advanced: bool,
    }

    impl CommandSpec {
        pub(crate) fn options(&self) -> impl Iterator<Item = &'static OptionSpec> + use<> {
            self.options.iter().flat_map(|group| group.iter())
        }
    }

    const VERBOSE: OptionSpec =
        OptionSpec::switch("--verbose", "show the complete route and evidence").short("-v");
    const COLOR: OptionSpec = OptionSpec::valued(
        "--color",
        OptionValue::Choices(&["auto", "always", "never"]),
        "set terminal colors",
    );
    const JSON: OptionSpec = OptionSpec::switch("--json", "emit JSON");
    const JSON_V2: OptionSpec = OptionSpec::switch("--json-v2", "emit JSON schema v2");
    const SCHEMA_VERSION: OptionSpec = OptionSpec::valued(
        "--schema-version",
        OptionValue::Choices(&["1", "2"]),
        "select the JSON schema version",
    );
    const NDJSON: OptionSpec = OptionSpec::switch(
        "--ndjson",
        "emit one compact schema-v2 JSON record per captured event",
    );

    /// Options removed by [`super::parse_global_arguments`] before dispatch.
    #[cfg(test)]
    pub(crate) const GLOBAL_OPTIONS: &[OptionSpec] =
        &[VERBOSE, COLOR, JSON, JSON_V2, SCHEMA_VERSION, NDJSON];
    const VERBOSE_OPTIONS: &[OptionSpec] = &[VERBOSE];
    const REPORT_OPTIONS: &[OptionSpec] = &[COLOR, JSON, JSON_V2, SCHEMA_VERSION];
    const NDJSON_OPTIONS: &[OptionSpec] = &[NDJSON];

    const INSPECT_OPTIONS: &[OptionSpec] = &[
        OptionSpec::valued(
            "--pid",
            OptionValue::Text("PID"),
            "inspect the process ancestry rooted at this PID",
        ),
        OptionSpec::switch("--focused", "inspect the focused window process"),
        OptionSpec::valued(
            "--instance",
            OptionValue::Text("ID"),
            "select a Hyprland instance explicitly",
        ),
    ];
    const LISTEN_OPTIONS: &[OptionSpec] = &[
        OptionSpec::switch("--repeat", "keep listening").short("-r"),
        OptionSpec::valued(
            "--timeout",
            OptionValue::Text("SECONDS"),
            "stop after this many seconds",
        ),
        OptionSpec::valued(
            "--count",
            OptionValue::Text("N"),
            "stop after this many reports",
        ),
        OptionSpec::valued(
            "--events",
            OptionValue::Choices(&["all"]),
            "include modifier and release events",
        ),
        OptionSpec::switch(
            "--suppress",
            "temporarily suppress compositor shortcuts during capture",
        ),
        OptionSpec::switch(
            "--no-suppress",
            "capture without suppressing compositor shortcuts",
        )
        .alternative(),
        OptionSpec::switch("--pass-through", "compatibility alias of --no-suppress")
            .alternative()
            .alias(),
        OptionSpec::switch(
            "--dry-run",
            "print the capture chain and selected backend without arming it",
        ),
        OptionSpec::switch(
            "--explain-capture",
            "show capture detection evidence and the capture chain",
        ),
        OptionSpec::switch("--terminal", "force terminal capture").short("-t"),
        OptionSpec::switch(
            "--evdev",
            "capture Linux input events before the compositor",
        )
        .short("-e"),
        OptionSpec::switch(
            "--window",
            "capture through a focused probe window (what the desktop delivers)",
        )
        .short("-w"),
        OptionSpec::valued(
            "--device",
            OptionValue::Path,
            "read one /dev/input/event device",
        ),
        OptionSpec::valued(
            "--output",
            OptionValue::Path,
            "write captured JSON reports to a file",
        ),
    ];
    const CAPABILITIES_OPTIONS: &[OptionSpec] = &[OptionSpec::switch(
        "--all",
        "list the complete adapter inventory",
    )];
    const INVENTORY_FILTER_OPTIONS: &[OptionSpec] = &[
        OptionSpec::valued("--key", OptionValue::Text("TEXT"), "filter by key"),
        OptionSpec::valued("--action", OptionValue::Text("TEXT"), "filter by action"),
        OptionSpec::valued("--source", OptionValue::Text("TEXT"), "filter by source"),
        OptionSpec::valued(
            "--device",
            OptionValue::Text("TEXT"),
            "filter by input device",
        ),
        OptionSpec::valued("--submap", OptionValue::Text("TEXT"), "filter by submap"),
    ];
    const SNAPSHOT_OPTIONS: &[OptionSpec] = &[OptionSpec::valued(
        "--output",
        OptionValue::Path,
        "write the snapshot to a file",
    )];

    /// Options of the bare `whykey <combination>` form.
    const BARE_OPTIONS: &[&[OptionSpec]] = &[VERBOSE_OPTIONS, REPORT_OPTIONS];

    pub(crate) const COMMANDS: &[CommandSpec] = &[
        CommandSpec {
            name: "inspect",
            operands: "<combination-or-sequence>",
            operand: Operand::Text,
            options: &[INSPECT_OPTIONS, VERBOSE_OPTIONS, REPORT_OPTIONS],
            summary: "explain one combination or sequence",
            advanced: false,
        },
        CommandSpec {
            name: "listen",
            operands: "",
            operand: Operand::None,
            options: &[
                LISTEN_OPTIONS,
                VERBOSE_OPTIONS,
                REPORT_OPTIONS,
                NDJSON_OPTIONS,
            ],
            summary: "capture a key and explain its path (Hyprland and Sway; Sway is pass-through-only)",
            advanced: false,
        },
        CommandSpec {
            name: "doctor",
            operands: "",
            operand: Operand::None,
            options: &[REPORT_OPTIONS],
            summary: "check session integrations",
            advanced: false,
        },
        CommandSpec {
            name: "capabilities",
            operands: "",
            operand: Operand::None,
            options: &[CAPABILITIES_OPTIONS, REPORT_OPTIONS],
            summary: "list applicable integrations (--all for the full inventory)",
            advanced: true,
        },
        CommandSpec {
            name: "bindings",
            operands: "",
            operand: Operand::None,
            options: &[INVENTORY_FILTER_OPTIONS, REPORT_OPTIONS],
            summary: "enumerate effective bindings",
            advanced: true,
        },
        CommandSpec {
            name: "conflicts",
            operands: "",
            operand: Operand::None,
            options: &[INVENTORY_FILTER_OPTIONS, REPORT_OPTIONS],
            summary: "group duplicate actions",
            advanced: true,
        },
        CommandSpec {
            name: "extension",
            operands: "<program> <combination>",
            operand: Operand::Text,
            options: &[REPORT_OPTIONS],
            summary: "ask one opt-in external diagnostic",
            advanced: true,
        },
        CommandSpec {
            name: "replay",
            operands: "<file>",
            operand: Operand::Files,
            options: &[VERBOSE_OPTIONS, REPORT_OPTIONS],
            summary: "re-render saved reports without injecting input",
            advanced: true,
        },
        CommandSpec {
            name: "snapshot",
            operands: "<combination>",
            operand: Operand::Text,
            options: &[SNAPSHOT_OPTIONS],
            summary: "save a static diagnostic snapshot without capturing input",
            advanced: true,
        },
        CommandSpec {
            name: "diff",
            operands: "<before> <after>",
            operand: Operand::Files,
            options: &[REPORT_OPTIONS],
            summary: "compare saved snapshots without querying the desktop",
            advanced: true,
        },
        CommandSpec {
            name: "shell-init",
            operands: "<bash|zsh|fish>",
            operand: Operand::Shells,
            options: &[],
            summary: "print shell integration",
            advanced: true,
        },
        CommandSpec {
            name: "completions",
            operands: "<bash|zsh|fish>",
            operand: Operand::Shells,
            options: &[],
            summary: "print shell completions",
            advanced: true,
        },
    ];

    pub(crate) const COMPLETION_SHELLS: &[&str] = &["bash", "zsh", "fish"];
    const COMPLETION_EXAMPLES: &[&str] = &["ctrl+left", "ctrl+z", "super+c"];

    fn command(name: &str) -> &'static CommandSpec {
        COMMANDS
            .iter()
            .find(|command| command.name == name)
            .expect("every dispatched command has a CommandSpec")
    }

    /// The usage line of one command, rendered from [`COMMANDS`].
    pub(crate) fn usage(name: &str) -> String {
        let command = command(name);
        let mut usage = format!("whykey {}", command.name);
        if !command.operands.is_empty() {
            usage.push(' ');
            usage.push_str(command.operands);
        }
        push_option_usage(&mut usage, command.options());
        usage
    }

    fn bare_usage() -> String {
        let mut usage = String::from("whykey <combination>");
        push_option_usage(
            &mut usage,
            BARE_OPTIONS.iter().flat_map(|group| group.iter()),
        );
        usage
    }

    fn push_option_usage<'a>(usage: &mut String, options: impl Iterator<Item = &'a OptionSpec>) {
        for option in options {
            if option.alternative && usage.ends_with(']') {
                usage.pop();
                usage.push('|');
            } else {
                usage.push_str(" [");
            }
            usage.push_str(option.long);
            match option.value {
                OptionValue::None => {}
                OptionValue::Text(placeholder) => {
                    usage.push(' ');
                    usage.push_str(placeholder);
                }
                OptionValue::Path => usage.push_str(" PATH"),
                OptionValue::Choices(choices) => {
                    usage.push(' ');
                    usage.push_str(&choices.join("|"));
                }
            }
            usage.push(']');
        }
    }

    /// Primary help leads with the common use; advanced commands follow.
    /// Every usage line is rendered from [`COMMANDS`], so help, the parser
    /// tests, and completions share one vocabulary.
    pub(crate) fn help_text() -> String {
        let mut output =
            String::from("whykey - explain where a key combination is handled\n\nUsage:\n");
        let _ = writeln!(output, "  {}", bare_usage());
        for command in COMMANDS.iter().filter(|command| !command.advanced) {
            let _ = writeln!(output, "  {}\n    {}", usage(command.name), command.summary);
        }
        output.push_str("\nAdvanced:\n");
        for command in COMMANDS.iter().filter(|command| command.advanced) {
            let _ = writeln!(output, "  {}\n    {}", usage(command.name), command.summary);
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
                "whykey() {{\n  local whykey_readline_bindings whykey_readline_macros whykey_readline_shell_bindings whykey_readline_variables whykey_readline_map\n  whykey_readline_bindings=\"$(builtin bind -p 2>/dev/null)\"\n  for whykey_readline_map in emacs emacs-standard emacs-meta vi vi-move vi-command vi-insertion; do\n    whykey_readline_bindings=\"$whykey_readline_bindings\n# WHYKEY_KEYMAP $whykey_readline_map\n$(builtin bind -m \"$whykey_readline_map\" -p 2>/dev/null)\"\ndone\n  whykey_readline_macros=\"$(builtin bind -S 2>/dev/null)\"\n  whykey_readline_shell_bindings=\"$(builtin bind -X 2>/dev/null)\"\n  whykey_readline_variables=\"$(builtin bind -V 2>/dev/null)\"\n  WHYKEY_READLINE_BINDINGS=\"$whykey_readline_bindings\" WHYKEY_READLINE_MACROS=\"$whykey_readline_macros\" WHYKEY_READLINE_SHELL_BINDINGS=\"$whykey_readline_shell_bindings\" WHYKEY_READLINE_VARIABLES=\"$whykey_readline_variables\" command {binary} \"$@\"\n}}"
            ),
            "zsh" => format!(
                "whykey() {{\n  local whykey_zle_bindings\n  whykey_zle_bindings=\"$(bindkey -lL main 2>/dev/null; bindkey -L 2>/dev/null; bindkey -LM vicmd 2>/dev/null)\"\n  WHYKEY_ZLE_BINDINGS=\"$whykey_zle_bindings\" command {binary} \"$@\"\n}}"
            ),
            "fish" => format!(
                "function whykey\n  set -lx WHYKEY_FISH_BINDINGS (bind --all 2>/dev/null | string collect)\n  set -lx WHYKEY_FISH_MODE $fish_bind_mode\n  command {binary} $argv\nend"
            ),
            _ => unreachable!("shell-init only accepts supported shells"),
        }
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    /// Shell completions rendered from [`COMMANDS`], so every shell offers
    /// exactly the options each command's usage line documents.
    pub(crate) fn completions(shell: &str) -> String {
        match shell {
            "bash" => bash_completions(),
            "zsh" => zsh_completions(),
            "fish" => fish_completions(),
            _ => unreachable!("completions only accepts supported shells"),
        }
    }

    fn bare_options() -> impl Iterator<Item = &'static OptionSpec> {
        BARE_OPTIONS.iter().flat_map(|group| group.iter())
    }

    /// Compatibility aliases are accepted but never offered.
    fn completed(
        options: impl Iterator<Item = &'static OptionSpec>,
    ) -> impl Iterator<Item = &'static OptionSpec> {
        options.filter(|option| !option.alias)
    }

    fn option_names(option: &OptionSpec) -> impl Iterator<Item = &'static str> + use<> {
        option.short.into_iter().chain([option.long])
    }

    fn bash_completions() -> String {
        let mut first_words: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        first_words.extend(COMPLETION_EXAMPLES);
        first_words.extend(completed(bare_options()).flat_map(option_names));
        let mut script = String::from(
            "_whykey_complete() {\n  local cur=\"${COMP_WORDS[COMP_CWORD]}\" prev=\"${COMP_WORDS[COMP_CWORD-1]}\"\n  if (( COMP_CWORD == 1 )); then\n",
        );
        let _ = writeln!(
            script,
            "    COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )",
            first_words.join(" ")
        );
        script.push_str("    return\n  fi\n  case \"${COMP_WORDS[1]}\" in\n");
        for command in COMMANDS {
            push_bash_case(
                &mut script,
                command.name,
                command.options(),
                command.operand,
            );
        }
        push_bash_case(&mut script, "*", bare_options(), Operand::Text);
        script.push_str("  esac\n}\ncomplete -F _whykey_complete whykey\n");
        script
    }

    fn push_bash_case(
        script: &mut String,
        pattern: &str,
        options: impl Iterator<Item = &'static OptionSpec>,
        operand: Operand,
    ) {
        let options: Vec<_> = completed(options).collect();
        let _ = writeln!(script, "    {pattern})");
        let mut valued = options
            .iter()
            .filter(|option| !matches!(option.value, OptionValue::None))
            .peekable();
        if valued.peek().is_some() {
            script.push_str("      case \"$prev\" in\n");
            for option in valued {
                let names: Vec<_> = option_names(option).collect();
                let action = match option.value {
                    OptionValue::Choices(choices) => format!(
                        "COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )",
                        choices.join(" ")
                    ),
                    OptionValue::Path => "COMPREPLY=( $(compgen -f -- \"$cur\") )".into(),
                    OptionValue::Text(_) | OptionValue::None => "COMPREPLY=()".into(),
                };
                let _ = writeln!(script, "        {}) {action}; return ;;", names.join("|"));
            }
            script.push_str("      esac\n");
        }
        let mut words: Vec<&str> = options
            .iter()
            .flat_map(|option| option_names(option))
            .collect();
        if operand == Operand::Shells {
            words.extend(COMPLETION_SHELLS);
        }
        let _ = writeln!(
            script,
            "      COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )",
            words.join(" ")
        );
        if operand == Operand::Files {
            script.push_str(
                "      [[ \"$cur\" == -* ]] || COMPREPLY+=( $(compgen -f -- \"$cur\") )\n",
            );
        }
        script.push_str("      ;;\n");
    }

    fn zsh_completions() -> String {
        let mut script = String::from(
            "#compdef whykey\n\n_whykey() {\n  if (( CURRENT == 2 )) && [[ $PREFIX != -* ]]; then\n    local -a commands\n    commands=(\n",
        );
        for command in COMMANDS {
            let _ = writeln!(
                script,
                "      {}",
                zsh_quote(&format!("{}:{}", command.name, command.summary))
            );
        }
        script.push_str("    )\n    _describe -t commands 'whykey command' commands\n");
        let _ = writeln!(script, "    compadd -- {}", COMPLETION_EXAMPLES.join(" "));
        script.push_str("    return\n  fi\n  case $words[2] in\n");
        for command in COMMANDS {
            let _ = writeln!(script, "    {})", command.name);
            script.push_str("      shift words\n      (( CURRENT-- ))\n");
            push_zsh_arguments(
                &mut script,
                command.options(),
                command.operands,
                command.operand,
            );
        }
        script.push_str("    *)\n");
        push_zsh_arguments(&mut script, bare_options(), "<combination>", Operand::Text);
        script.push_str("  esac\n}\n\n_whykey \"$@\"\n");
        script
    }

    fn push_zsh_arguments(
        script: &mut String,
        options: impl Iterator<Item = &'static OptionSpec>,
        operands: &str,
        operand: Operand,
    ) {
        script.push_str("      _arguments -s");
        for option in completed(options) {
            let help = option
                .help
                .replace('\\', "\\\\")
                .replace('[', "\\[")
                .replace(']', "\\]");
            let value = match option.value {
                OptionValue::None => String::new(),
                OptionValue::Text(placeholder) => format!(":{placeholder}:"),
                OptionValue::Path => ":path:_files".into(),
                OptionValue::Choices(choices) => format!(
                    ":{}:({})",
                    option.long.trim_start_matches('-'),
                    choices.join(" ")
                ),
            };
            for name in option_names(option) {
                let _ = write!(
                    script,
                    " \\\n        {}",
                    zsh_quote(&format!("{name}[{help}]{value}"))
                );
            }
        }
        let operand_spec = match operand {
            Operand::None => None,
            Operand::Text => Some(format!("*:{}:", operands.replace(':', "\\:"))),
            Operand::Files => Some(format!("*:{}:_files", operands.replace(':', "\\:"))),
            Operand::Shells => Some(format!("1:shell:({})", COMPLETION_SHELLS.join(" "))),
        };
        if let Some(spec) = operand_spec {
            let _ = write!(script, " \\\n        {}", zsh_quote(&spec));
        }
        script.push_str("\n      ;;\n");
    }

    fn zsh_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn fish_completions() -> String {
        let names = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<Vec<_>>()
            .join(" ");
        let mut script = String::from("complete -c whykey -f\n");
        for command in COMMANDS {
            let _ = writeln!(
                script,
                "complete -c whykey -n '__fish_use_subcommand' -a {} -d {}",
                command.name,
                fish_quote(command.summary)
            );
        }
        let _ = writeln!(
            script,
            "complete -c whykey -n '__fish_use_subcommand' -a {} -d 'example combination'",
            fish_quote(&COMPLETION_EXAMPLES.join(" "))
        );
        let bare_condition = format!("not __fish_seen_subcommand_from {names}");
        for option in completed(bare_options()) {
            push_fish_option(&mut script, &bare_condition, option);
        }
        for command in COMMANDS {
            let condition = format!("__fish_seen_subcommand_from {}", command.name);
            for option in completed(command.options()) {
                push_fish_option(&mut script, &condition, option);
            }
            match command.operand {
                Operand::Shells => {
                    let _ = writeln!(
                        script,
                        "complete -c whykey -n {} -a {}",
                        fish_quote(&condition),
                        fish_quote(&COMPLETION_SHELLS.join(" "))
                    );
                }
                Operand::Files => {
                    let _ = writeln!(
                        script,
                        "complete -c whykey -n {} -F",
                        fish_quote(&condition)
                    );
                }
                Operand::None | Operand::Text => {}
            }
        }
        script
    }

    fn push_fish_option(script: &mut String, condition: &str, option: &OptionSpec) {
        let _ = write!(script, "complete -c whykey -n {}", fish_quote(condition));
        if let Some(short) = option.short {
            let _ = write!(script, " -s {}", short.trim_start_matches('-'));
        }
        let _ = write!(script, " -l {}", option.long.trim_start_matches('-'));
        match option.value {
            OptionValue::None => {}
            OptionValue::Text(_) => script.push_str(" -x"),
            OptionValue::Path => script.push_str(" -r -F"),
            OptionValue::Choices(choices) => {
                let _ = write!(script, " -x -a {}", fish_quote(&choices.join(" ")));
            }
        }
        let _ = writeln!(script, " -d {}", fish_quote(option.help));
    }

    fn fish_quote(value: &str) -> String {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

#[cfg(test)]
pub(crate) use help::COMMANDS;
pub(crate) use help::{COMPLETION_SHELLS, completions, help_text, shell_init, usage};

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

    fn strings(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn conflicting_schema_selections_are_a_usage_error() {
        for conflict in [
            &["--json-v2", "--schema-version", "1", "ctrl+x"][..],
            &["--schema-version", "1", "--json-v2"],
            &["--schema-version", "2", "--schema-version", "1"],
        ] {
            assert!(
                parse_global_arguments(strings(conflict)).is_err(),
                "{conflict:?} must be rejected"
            );
        }
        for (repeat, version) in [
            (&["--json-v2", "--schema-version", "2"][..], 2),
            (&["--json-v2", "--json-v2"], 2),
            (&["--schema-version", "1", "--schema-version", "1"], 1),
        ] {
            let parsed = parse_global_arguments(strings(repeat)).unwrap();
            assert_eq!(parsed.schema_version, version, "{repeat:?}");
        }
    }

    /// Help and completions render from the command table; this keeps the
    /// parsers honest about every option the table documents.
    #[test]
    fn every_documented_option_is_accepted_by_its_parser() {
        use help::{GLOBAL_OPTIONS, OptionValue};
        for command in help::COMMANDS {
            for option in command.options() {
                for name in option.short.into_iter().chain([option.long]) {
                    let mut arguments = vec![name.to_owned()];
                    match option.value {
                        OptionValue::None => {}
                        OptionValue::Text(_) => arguments.push("1".into()),
                        OptionValue::Path => arguments.push("/dev/null".into()),
                        OptionValue::Choices(choices) => arguments.push(choices[0].into()),
                    }
                    let accepted = if GLOBAL_OPTIONS
                        .iter()
                        .any(|global| global.long == option.long)
                    {
                        parse_global_arguments(arguments.into_iter().map(Into::into).collect())
                            .is_ok_and(|parsed| parsed.arguments.is_empty())
                    } else {
                        match command.name {
                            "inspect" => {
                                arguments.push("ctrl+x".into());
                                parse_inspect_arguments(arguments).is_ok()
                            }
                            "listen" => matches!(
                                parse_listen_arguments(
                                    arguments,
                                    true,
                                    false,
                                    false,
                                    ColorChoice::Never,
                                    2
                                ),
                                Ok(Parsed::Run(_))
                            ),
                            "capabilities" => {
                                matches!(parse_capabilities_arguments(arguments), Ok(true))
                            }
                            "bindings" | "conflicts" => {
                                parse_inventory_filters(arguments, "").is_ok()
                            }
                            "snapshot" => {
                                arguments.insert(0, "ctrl+x".into());
                                let arguments = arguments.into_iter().map(Into::into).collect();
                                matches!(parse_snapshot_arguments(arguments), Ok(Parsed::Run(_)))
                            }
                            other => panic!("{other} documents {name} but has no parser check"),
                        }
                    };
                    assert!(
                        accepted,
                        "{} rejects documented option {name}",
                        command.name
                    );
                }
            }
        }
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

    #[test]
    fn listen_rejects_suppress_with_backends_that_cannot_suppress() {
        // Each case is refused during argument validation, before any capture
        // backend (terminal, evdev, or compositor) is opened.
        for arguments in [
            vec!["--terminal", "--suppress"],
            vec!["--suppress", "--evdev"],
            vec!["--suppress", "--device", "/dev/input/event0"],
        ] {
            let code = dispatch(
                "listen".into(),
                arguments.iter().map(OsString::from).collect(),
                false,
                false,
                false,
                ColorChoice::Never,
                2,
            );
            assert_eq!(code, ExitCode::from(2), "{arguments:?}");
        }
    }
}
