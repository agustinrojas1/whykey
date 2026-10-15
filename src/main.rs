use std::env;
use std::process::ExitCode;

mod cli;

#[cfg(test)]
use cli::{COMMANDS, COMPLETION_SHELLS};
use cli::{
    DoctorState, command_program_available, command_succeeds, completions, detect_extension,
    detect_nvim_server, detected_terminal_program, doctor_next_steps, help_text, shell_init,
    shell_snapshot_available,
};

use whykey::bindings;
use whykey::capabilities;
use whykey::command;
use whykey::conflicts;
use whykey::diff;
use whykey::extensions;
use whykey::focus;
use whykey::key::KeyCombo;
use whykey::layers::{Outcome, hyprland, inspect_default_chain, inspect_sequence, programmable};
use whykey::listen;
use whykey::replay;
use whykey::report;
use whykey::schema;
use whykey::snapshot;
use whykey::style::{ColorChoice, RenderOptions};

/// One source of truth for command vocabulary. Help usage lines and shell
/// completions render from [`cli::help::COMMANDS`]; the parser tests in
/// `cli` prove every listed option is accepted.
fn main() -> ExitCode {
    let parsed = match cli::parse_global_arguments(env::args_os().skip(1).collect()) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let cli::GlobalArguments {
        arguments: raw_arguments,
        json,
        ndjson,
        verbose,
        color,
        ascii,
        schema_version,
    } = parsed;
    whykey::style::set_force_ascii(ascii);
    let mut arguments = raw_arguments.into_iter();
    let Some(argument) = arguments.next() else {
        eprintln!("{}", help_text());
        return ExitCode::from(2);
    };
    let argument = match cli::utf8_argument(argument) {
        Ok(argument) => argument,
        Err(code) => return code,
    };
    // A closed stdout (`whykey --help | head -1`) ends one-shot commands
    // quietly with status 141, as SIGPIPE ends other Unix filters, not with a
    // "failed printing" panic. SIGPIPE itself stays ignored (Rust's default):
    // a SIG_DFL disposition would also kill whykey when an extension closes
    // its stdin before reading the request, instead of reporting that error.
    // `listen` keeps the plain panic: its terminal, kitty-flag, and
    // native-hook guards restore state in `Drop`, which an exit would skip.
    if argument != "listen" {
        exit_quietly_on_closed_stdout();
    }

    if matches!(argument.as_str(), "-h" | "--help") {
        println!("{}", help_text());
        return ExitCode::SUCCESS;
    }
    if matches!(argument.as_str(), "-V" | "--version") {
        println!("whykey {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let code = cli::dispatch(
        argument,
        arguments.collect(),
        json,
        ndjson,
        verbose,
        color,
        schema_version,
    );
    // A replay that left recorded answers unused is not a clean run.
    whykey::recording::finish(code)
}

/// Turns `println!`'s "failed printing to stdout: Broken pipe" panic into a
/// silent exit with the shell's SIGPIPE status, leaving other panics alone.
fn exit_quietly_on_closed_stdout() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied());
        let broken_pipe = std::io::Error::from_raw_os_error(libc::EPIPE).to_string();
        if let Some(message) = message {
            if message.starts_with("failed printing to stdout") && message.ends_with(&broken_pipe) {
                std::process::exit(128 + libc::SIGPIPE);
            }
        }
        default_hook(info);
    }));
}

fn run_listen(options: listen::Options) -> ExitCode {
    match listen::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("whykey listen: {error}");
            ExitCode::from(1)
        }
    }
}

fn run_inspect(
    arguments: Vec<String>,
    json: bool,
    verbose: bool,
    color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let cli::InspectArguments {
        sequence,
        target_pid,
        focused,
        instance,
    } = match cli::parse_inspect_arguments(arguments) {
        Ok(parsed) => parsed,
        Err(code) => return code,
    };

    let inspect_with_budget = || {
        command::with_deadline(command::configured_diagnostic_timeout(), || {
            let focused_target = focused.then(focus::resolve).transpose()?;
            let resolved_pid = focused_target
                .as_ref()
                .map(|target| target.pid)
                .or(target_pid);
            let resolved_source = focused_target.as_ref().map(|target| target.source.as_str());
            let target = resolved_pid.map(|pid| (pid, resolved_source));
            let reports = inspect_sequence(sequence.as_slice(), target);
            Ok::<_, String>((reports, resolved_pid))
        })
    };
    let reports_result = match instance.as_deref() {
        Some(instance) => hyprland::with_instance_override(instance, inspect_with_budget),
        None => inspect_with_budget(),
    };
    let (reports, resolved_pid) = match reports_result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("whykey inspect --focused: {error}");
            return ExitCode::from(1);
        }
    };
    let has_explicit_target = target_pid.is_some() || focused;
    if let Some(pid) = resolved_pid.filter(|pid| !focus::shares_terminal_with_whykey(*pid)) {
        // The terminal emulator and TTY driver layers read whykey's own
        // environment and /dev/tty; only the application and shell layers
        // follow the target. Say so instead of mixing the two silently.
        eprintln!(
            "note: process {pid} is not on whykey's terminal; the terminal emulator and TTY \
             driver rows describe the terminal whykey runs in, the application and shell \
             rows describe process {pid}"
        );
    }
    let unavailable = reports
        .iter()
        .any(|steps| cli::inspection_failed(steps, has_explicit_target));

    if json {
        print!(
            "{}",
            report::render_sequence_json(&sequence, &reports, schema_version)
        );
    } else {
        print!(
            "{}",
            report::render_sequence_with_options(
                &sequence,
                &reports,
                RenderOptions::cli(color, verbose),
            )
        );
    }

    if unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_capabilities(json: bool, all: bool, _color: ColorChoice, schema_version: u8) -> ExitCode {
    let environment = whykey::environment::Environment::collect();
    let capabilities = command::with_deadline(command::configured_diagnostic_timeout(), || {
        capabilities::current_with_environment(&environment)
    });
    if json {
        print!(
            "{}",
            capabilities::render_json_with_environment(&capabilities, schema_version, &environment)
        );
    } else {
        print!(
            "{}",
            capabilities::render_text_with_options(
                &capabilities,
                all,
                RenderOptions::cli(_color, false),
            )
        );
    }
    ExitCode::SUCCESS
}

fn run_bindings(
    json: bool,
    schema_version: u8,
    filters: cli::InventoryFilters,
    _color: ColorChoice,
) -> ExitCode {
    let inventory = command::with_deadline(command::configured_diagnostic_timeout(), || {
        bindings::filter(
            bindings::current(),
            filters.key.as_deref(),
            filters.action.as_deref(),
            filters.source.as_deref(),
            filters.device.as_deref(),
            filters.submap.as_deref(),
        )
    });
    if json {
        print!("{}", bindings::render_json(&inventory, schema_version));
    } else {
        print!(
            "{}",
            bindings::render_text_with_options(&inventory, RenderOptions::cli(_color, false))
        );
    }
    if inventory.bindings.is_empty() && !inventory.unavailable.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_conflicts(
    json: bool,
    schema_version: u8,
    filters: cli::InventoryFilters,
    _color: ColorChoice,
) -> ExitCode {
    let report = command::with_deadline(command::configured_diagnostic_timeout(), || {
        conflicts::filter(
            conflicts::current(),
            filters.key.as_deref(),
            filters.action.as_deref(),
            filters.source.as_deref(),
            filters.device.as_deref(),
            filters.submap.as_deref(),
        )
    });
    if json {
        print!("{}", conflicts::render_json(&report, schema_version));
    } else {
        print!(
            "{}",
            conflicts::render_text_with_options(&report, RenderOptions::cli(_color, false))
        );
    }
    if report.conflicts.is_empty() && !report.unavailable.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_extension(
    program: &std::ffi::OsStr,
    input: String,
    json: bool,
    _color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let key = match KeyCombo::parse_query(&input) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}\n\nTry 'whykey --help'.");
            return ExitCode::from(2);
        }
    };
    let result = command::with_deadline(command::configured_diagnostic_timeout(), || {
        extensions::inspect(program, &key)
    });
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            eprintln!("whykey extension: {error}");
            return ExitCode::from(1);
        }
    };
    if json {
        print!("{}", extensions::render_json(&result, &key, schema_version));
    } else {
        print!(
            "{}",
            extensions::render_text_with_options(&result, &key, RenderOptions::cli(_color, false),)
        );
    }
    if result.layer.outcome == Outcome::Unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_replay(
    path: std::path::PathBuf,
    json: bool,
    verbose: bool,
    _color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let documents = match replay::load(&path) {
        Ok(documents) => documents,
        Err(error) => {
            eprintln!("whykey replay: {error}");
            return ExitCode::from(1);
        }
    };
    if json {
        print!("{}", replay::render_json(&documents, schema_version));
    } else {
        print!(
            "{}",
            replay::render_text_with_options(&documents, RenderOptions::cli(_color, verbose))
        );
    }
    ExitCode::SUCCESS
}

fn run_snapshot(request: cli::SnapshotRequest) -> ExitCode {
    let cli::SnapshotRequest {
        combination,
        output_path,
    } = request;
    let key = match KeyCombo::parse_query(&combination) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(2);
        }
    };
    let results = command::with_deadline(command::configured_diagnostic_timeout(), || {
        inspect_default_chain(&key)
    });
    // Same exit rule as `whykey KEY`: a report with an unavailable layer is
    // still a completed inspection unless nothing could be inspected.
    let failed = cli::inspection_failed(&results, false);
    let value = snapshot::create(&key, &results);
    if let Some(path) = output_path {
        let path = std::path::Path::new(&path);
        if let Err(error) = snapshot::save(path, &value) {
            eprintln!(
                "whykey snapshot: could not write {}: {error}",
                path.display()
            );
            return ExitCode::from(1);
        }
        eprintln!("whykey snapshot: wrote {}", path.display());
    } else {
        print!("{}", snapshot::render(&value));
    }
    if failed {
        eprintln!(
            "whykey snapshot: exit 1 because the inspection is incomplete (every layer was \
             unavailable or the diagnostic budget expired); the snapshot still records it"
        );
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_diff(
    paths: Vec<std::path::PathBuf>,
    json: bool,
    _color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let load_one = |path: &std::path::Path| -> Result<serde_json::Value, String> {
        let documents = replay::load(path).map_err(|error| error.to_string())?;
        if documents.len() != 1 {
            return Err(format!(
                "{} contains {} reports; expected exactly one",
                path.display(),
                documents.len()
            ));
        }
        Ok(documents
            .into_iter()
            .next()
            .expect("one report was validated"))
    };
    let before = match load_one(&paths[0]) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("whykey diff: could not read before report: {error}");
            return ExitCode::from(1);
        }
    };
    let after = match load_one(&paths[1]) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("whykey diff: could not read after report: {error}");
            return ExitCode::from(1);
        }
    };
    let changes = diff::compare(&before, &after);
    if json {
        print!("{}", diff::render_json(&changes, schema_version));
    } else {
        print!(
            "{}",
            diff::render_text_with_options(&changes, RenderOptions::cli(_color, false))
        );
    }
    ExitCode::SUCCESS
}

fn run_doctor(json: bool, _color: ColorChoice, schema_version: u8) -> ExitCode {
    // One snapshot per command: desktop discovery runs once here.
    let environment = whykey::environment::Environment::collect();
    let tty = environment.tty_available;
    let ssh = environment.ssh;
    let generic_compositor = environment.desktop("compositor").applicable;
    let compositor_context = environment.compositor_context();
    let ghostty = command_succeeds("ghostty", &["+list-keybinds", "--plain"]);
    let xkb = command_succeeds("xkbcli", &["--version"]);
    let terminal_program = detected_terminal_program();
    let non_ghostty_terminal = terminal_program.as_deref().is_some_and(|program| {
        !matches!(
            program.to_ascii_lowercase().as_str(),
            "ghostty" | "xterm-ghostty"
        )
    });
    let generic_terminal_fallback = non_ghostty_terminal || terminal_program.is_none();
    let terminal_adapter = environment.terminal;
    let shell = environment.shell.clone();
    let shell_snapshot = shell_snapshot_available(&shell);
    let evdev = listen::evdev_available();
    let hyprland_native = whykey::capabilities::hyprland_native_capture();
    let native_compositor = hyprland_native.is_ok();
    // Hyprland IPC answers but the key hook cannot be installed (a hyprlang
    // `.conf` config): `listen` would fall back, so doctor must not say OK.
    let hyprland_hook_unavailable = hyprland_native.err().flatten();
    let native_sway = whykey::sway_capture::probe_available();
    let native_available = native_compositor || native_sway;
    let native_backend = if native_compositor || hyprland_hook_unavailable.is_some() {
        whykey::capture::NativeBackendId::Hyprland.display()
    } else {
        whykey::capture::NativeBackendId::Sway.display()
    };
    let native_attempts = environment
        .compositor_candidates
        .iter()
        .filter_map(|candidate| {
            let entry = whykey::registry::DESKTOPS
                .iter()
                .find(|entry| entry.id == candidate.id)?;
            entry.capture.map(|_| {
                serde_json::json!({
                    "backend": entry.display,
                    "available": candidate.ipc && match candidate.id {
                        "hyprland" => native_compositor,
                        "sway" => native_sway,
                        _ => false,
                    },
                    "reason": if candidate.ipc && match candidate.id {
                        "hyprland" => native_compositor,
                        "sway" => native_sway,
                        _ => false,
                    } {
                        "IPC connection accepted".to_owned()
                    } else if let (true, "hyprland", Some(reason)) =
                        (candidate.ipc, candidate.id, hyprland_hook_unavailable.as_deref())
                    {
                        reason.to_owned()
                    } else {
                        "IPC connection unavailable".to_owned()
                    },
                })
            })
        })
        .collect::<Vec<_>>();
    let native_cause = if native_available {
        whykey::doctor::Cause::Available
    } else if hyprland_hook_unavailable.is_some() {
        whykey::doctor::Cause::ApiDoesNotExpose
    } else if native_attempts.is_empty() {
        whykey::doctor::Cause::NotInstalled
    } else {
        whykey::doctor::Cause::IpcUnreachable
    };
    let evdev_devices = listen::evdev_devices();
    let evdev_scan = whykey::doctor::EvdevScan::classify(
        std::path::Path::new("/dev/input"),
        evdev || !evdev_devices.is_empty(),
        evdev,
    );
    let evdev_cause = evdev_scan.cause();
    let evdev_acl = if evdev {
        None
    } else {
        whykey::doctor::evdev_acl_command(&evdev_devices)
    };
    let evdev_next = evdev_acl
        .clone()
        .or_else(|| evdev_scan.next_step().map(str::to_owned));
    let remappers = environment.remappers.clone();
    let ime = environment.ime.clone();
    let mut multiplexer_names = Vec::new();
    if environment.tmux {
        multiplexer_names.push("tmux");
    }
    if environment.screen {
        multiplexer_names.push("screen");
    }
    if environment.zellij {
        multiplexer_names.push("zellij");
    }
    let multiplexer_display = &environment.multiplexer;
    let nvim_extension = detect_extension("whykey-nvim");
    let nvim_server = detect_nvim_server();
    let runtime_readers = whykey::runtime_readers::current();
    let observation_taps = whykey::doctor::observation_taps(&whykey::doctor::TapInputs {
        native: native_available.then_some(native_backend),
        wayland: (whykey::probe::available_display() == Some("Wayland"))
            .then(|| whykey::recording::env_var("WAYLAND_DISPLAY").ok())
            .flatten(),
        x11: whykey::recording::env_var("DISPLAY")
            .ok()
            .filter(|value| !value.is_empty()),
        xwayland: whykey::layers::x11_grab::xwayland_session(),
        remote: ssh,
        tty,
        evdev_readable: evdev,
        evdev_detail: evdev_scan.detail(),
        evdev_enable: evdev_next.clone(),
    });
    let layer_sources = doctor_layer_sources(&DoctorLayerFacts {
        environment: &environment,
        x11_grab: observation_taps
            .iter()
            .any(|tap| tap.id == "x11-grab" && tap.available),
        ghostty,
        shell: &shell,
        shell_snapshot,
        nvim_server: nvim_server.as_deref(),
    });

    let compositor_context_available = ssh
        || generic_compositor
        || !environment.extension_candidates.is_empty()
        || whykey::registry::DESKTOPS.iter().any(|entry| {
            entry.doctor.is_some() && {
                let status = environment.desktop(entry.id);
                status.applicable && status.ipc
            }
        });
    let doctor_state = DoctorState {
        tty,
        compositor_context: compositor_context_available,
        ghostty,
        generic_terminal: generic_terminal_fallback,
        xkb,
        evdev: evdev_scan,
        evdev_acl,
        shell_snapshot,
        ssh,
        remapper: !remappers.is_empty(),
        ime: !ime.is_empty(),
    };
    let next_steps = doctor_next_steps(&doctor_state);

    if json {
        // Desktop detection and IPC status come from the registry; the
        // remaining checks stay explicit because they are not desktop
        // adapters.
        let mut desktop_checks = serde_json::Map::new();
        for entry in whykey::registry::DESKTOPS {
            let Some(doctor) = entry.doctor else {
                continue;
            };
            let status = environment.desktop(entry.id);
            let ipc = status.applicable && status.ipc;
            let mut check = serde_json::json!({"ipc": ipc, "applicable": status.applicable});
            if entry.id == "programmable" {
                check["desktop"] = match programmable::detect() {
                    Some(wm) => serde_json::json!(wm.name()),
                    None => serde_json::Value::Null,
                };
            }
            desktop_checks.insert(doctor.json_key.to_owned(), check);
        }
        let extension_checks = environment
            .extension_adapters
            .iter()
            .map(|adapter| {
                serde_json::json!({
                    "id": adapter.id,
                    "manifest": adapter.manifest_path,
                    "applicable": whykey::extension_adapters::applicable_with_context(
                        adapter,
                        &environment.compositor_context,
                    ),
                    "bindings_cmd_reachable": command_program_available(&adapter.bindings_cmd),
                    "last_error": serde_json::Value::Null,
                })
            })
            .collect::<Vec<_>>();
        let manifest_warnings = environment
            .extension_warnings
            .iter()
            .map(|warning| {
                serde_json::json!({
                    "manifest": warning.path,
                    "warning": warning.message,
                })
            })
            .collect::<Vec<_>>();
        let mut legacy_value = serde_json::json!({
            "schema_version": 1,
            "tty": {"available": tty},
            "compositor": {
                "applicable": generic_compositor,
                "desktop": compositor_context.desktop,
                "session_type": compositor_context.session_type,
                "display_server": compositor_context.display_server,
            },
            "ghostty": {
                "applicable": !generic_terminal_fallback,
                "effective_keybinds": ghostty,
                "generic_terminal_fallback": generic_terminal_fallback,
            },
            "xkb": {"compiler": xkb},
            "evdev": {
                "available": evdev,
                "devices": evdev_devices,
                "cause": evdev_cause,
                "detail": evdev_scan.detail(),
                "next_check": evdev_next.as_deref().unwrap_or(evdev_cause.next_check()),
            },
            "native-compositor": {
                "available": native_available,
                "backend": native_available.then_some(native_backend),
                "cause": native_cause,
                "detail": hyprland_hook_unavailable,
                "next_check": native_cause.next_check(),
                "attempted": native_attempts.clone(),
            },
            "remappers": remappers,
            "ime": ime,
            "terminal_program": terminal_program,
            "terminal_adapter": terminal_adapter,
            "shell": shell,
            "shell_snapshot": shell_snapshot,
            "multiplexer": multiplexer_display,
            "ssh": ssh,
            "extensions": {
                "whykey_nvim": {
                    "installed": nvim_extension,
                    "server": nvim_server,
                }
            },
            "extension-adapters": extension_checks,
            "extension-adapter-warnings": manifest_warnings,
            "runtime-readers": runtime_readers,
        });
        legacy_value
            .as_object_mut()
            .expect("doctor checks are an object")
            .extend(desktop_checks);
        let value = if schema_version == 2 {
            serde_json::json!({
                "schema_version": 2,
                "operation": "doctor",
                "context": schema::context_for_environment(&environment),
                "checks": legacy_value,
                "observation_taps": observation_taps,
                "layer_sources": layer_sources,
                "next_steps": next_steps.clone(),
            })
        } else {
            legacy_value
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("doctor is serializable")
        );
    } else {
        let render_options = RenderOptions::cli(_color, false);
        let facts = DoctorRows {
            environment: &environment,
            tty,
            ssh,
            xkb,
            ghostty,
            evdev,
            generic_compositor,
            generic_terminal_fallback,
            terminal_program: terminal_program.as_deref(),
            terminal_adapter,
            native_available,
            native_backend,
            native_cause: native_cause.label(),
            native_next: native_cause.next_check(),
            hyprland_hook_unavailable: hyprland_hook_unavailable.as_deref(),
            native_attempts: &native_attempts,
            evdev_scan,
            evdev_devices: &evdev_devices,
            evdev_commands: whykey::doctor::evdev_acl_commands(&evdev_devices).filter(|_| !evdev),
            runtime_readers: &runtime_readers,
            observation_taps: &observation_taps,
            layer_sources: &layer_sources,
            remappers: &remappers,
            ime: &ime,
            shell: &shell,
            shell_snapshot,
            multiplexer_names: &multiplexer_names,
            nvim_extension,
            nvim_server: nvim_server.as_deref(),
        };
        let context = if render_options.context {
            facts.context()
        } else {
            String::new()
        };
        print!(
            "{}",
            whykey::doctor::render_report(&context, &facts.groups(), render_options)
        );
    }
    if tty && compositor_context_available && (ghostty || generic_terminal_fallback) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// What doctor found, as the inputs to its human report.
struct DoctorRows<'a> {
    environment: &'a whykey::environment::Environment,
    tty: bool,
    ssh: bool,
    xkb: bool,
    ghostty: bool,
    evdev: bool,
    generic_compositor: bool,
    generic_terminal_fallback: bool,
    terminal_program: Option<&'a str>,
    terminal_adapter: Option<&'a str>,
    native_available: bool,
    native_backend: &'a str,
    native_cause: &'a str,
    native_next: &'a str,
    hyprland_hook_unavailable: Option<&'a str>,
    native_attempts: &'a [serde_json::Value],
    evdev_scan: whykey::doctor::EvdevScan,
    evdev_devices: &'a [whykey::listen::EvdevDeviceInfo],
    evdev_commands: Option<(String, String)>,
    runtime_readers: &'a [whykey::runtime_readers::ReaderStatus],
    observation_taps: &'a [whykey::doctor::ObservationTap],
    layer_sources: &'a [whykey::doctor::LayerSource],
    remappers: &'a [whykey::remapper::Detection],
    ime: &'a [whykey::ime::Detection],
    shell: &'a str,
    shell_snapshot: bool,
    multiplexer_names: &'a [&'a str],
    nvim_extension: bool,
    nvim_server: Option<&'a str>,
}

impl DoctorRows<'_> {
    /// `Hyprland · Wayland · bash` for the header.
    fn context(&self) -> String {
        let context = &self.environment.compositor_context;
        let session = context
            .session_type
            .as_deref()
            .or(context.display_server.as_deref())
            .map(|value| match value {
                "wayland" => "Wayland",
                "x11" => "X11",
                "tty" => "TTY",
                other => other,
            });
        let shell = self
            .shell
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty());
        let desktop = self
            .environment
            .compositor_candidates
            .iter()
            .find(|candidate| candidate.ipc)
            .and_then(|candidate| {
                whykey::registry::DESKTOPS
                    .iter()
                    .find(|entry| entry.id == candidate.id)
                    .map(|entry| entry.display)
            })
            .or(context.desktop.as_deref());
        [desktop, session, shell]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
    }

    fn groups(&self) -> Vec<whykey::doctor::Group> {
        use whykey::doctor::{Group, Row};
        use whykey::style::Check;
        let state = |ok: bool| if ok { Check::Ok } else { Check::Fail };

        let mut capture = Vec::new();
        capture.push(if self.tty {
            Row::new(Check::Ok, "controlling TTY")
        } else {
            Row::new(Check::Fail, "controlling TTY").reason("run whykey inside a terminal")
        });
        let mut native = Row::new(
            state(self.native_available),
            format!("{} native capture", self.native_backend),
        );
        if !self.native_available {
            native = native.reason(match self.hyprland_hook_unavailable {
                Some(reason) => reason.to_owned(),
                None => format!("{}: {}", self.native_cause, self.native_next),
            });
            if !self.native_attempts.is_empty() {
                let attempted = self
                    .native_attempts
                    .iter()
                    .filter_map(|entry| entry.get("backend").and_then(serde_json::Value::as_str))
                    .collect::<Vec<_>>()
                    .join(", ");
                native = native.note(format!("attempted: {attempted}"));
            }
        }
        capture.push(native);
        let mut evdev =
            Row::new(state(self.evdev), "evdev keyboards").reason(self.evdev_scan.detail());
        if let Some((grant, undo)) = &self.evdev_commands {
            evdev = evdev.fix(grant.clone()).undo(undo.clone());
            evdev =
                evdev.note("the grant is lost when the device is replugged or the system reboots");
        } else if !self.evdev {
            if let Some(step) = self.evdev_scan.next_step() {
                evdev = evdev.note(step);
            }
        }
        for device in self.evdev_devices.iter().take(5) {
            let status = if device.readable {
                "readable"
            } else {
                device.error.as_deref().unwrap_or("not readable")
            };
            evdev = evdev.note(format!(
                "{} \u{2014} {} ({status})",
                device.path, device.name
            ));
        }
        if self.evdev_devices.len() > 5 {
            evdev = evdev.note(format!("\u{2026} {} more", self.evdev_devices.len() - 5));
        }
        capture.push(evdev);
        for tap in self.observation_taps {
            if matches!(tap.id, "probe-window" | "x11-grab") {
                capture.push(Row::new(state(tap.available), tap.name).reason(tap.detail.clone()));
            }
        }

        let mut live = Vec::new();
        for reader in self.runtime_readers {
            use whykey::runtime_readers::Availability;
            let check = match reader.availability {
                Availability::Available => Check::Ok,
                Availability::MalformedResponse => Check::Unknown,
                Availability::Unavailable | Availability::ApiDoesNotExpose => Check::Skip,
            };
            let reason = if check == Check::Ok {
                String::new()
            } else {
                reader.evidence.clone()
            };
            live.push(Row::new(check, reader.id).reason(reason));
        }
        let selected = whykey::registry::DESKTOPS
            .iter()
            .find(|entry| entry.doctor.is_some() && self.environment.desktop(entry.id).applicable);
        if let Some(entry) = selected {
            let doctor = entry.doctor.expect("selected desktop has doctor metadata");
            let status = self.environment.desktop(entry.id);
            let ipc = status.applicable && status.ipc;
            let name = if entry.id == "programmable" {
                programmable::detect().map_or("programmable X11 WM", |wm| wm.name())
            } else {
                doctor.check
            };
            let row = Row::new(state(ipc), name);
            live.push(if ipc { row } else { row.reason(doctor.hint) });
        } else if self.ssh {
            live.push(
                Row::new(Check::Skip, "Hyprland IPC").reason("not applicable in this SSH session"),
            );
        } else if self.generic_compositor {
            let context = &self.environment.compositor_context;
            let desktop = context.desktop.as_deref().unwrap_or("unknown desktop");
            let session = context
                .session_type
                .as_deref()
                .or(context.display_server.as_deref())
                .unwrap_or("unknown display session");
            live.push(Row::new(Check::Fail, "desktop compositor").reason(format!(
                "{desktop} ({session}) detected, but no read-only adapter is available"
            )));
        } else {
            live.push(Row::new(Check::Fail, "compositor session").reason("not detected"));
        }
        for adapter in &self.environment.extension_adapters {
            let applicable = whykey::extension_adapters::applicable_with_context(
                adapter,
                &self.environment.compositor_context,
            );
            let reachable = command_program_available(&adapter.bindings_cmd);
            let (check, reason) = if !applicable {
                (Check::Skip, "environment or desktop hints do not match")
            } else if !reachable {
                (Check::Fail, "bindings command executable was not found")
            } else {
                (
                    Check::Ok,
                    "manifest applies; commands are queried only when needed",
                )
            };
            live.push(Row::new(check, format!("{} manifest", adapter.display)).reason(reason));
        }
        for warning in &self.environment.extension_warnings {
            live.push(Row::new(Check::Fail, "extension manifest").reason(format!(
                "{} ({})",
                warning.path.display(),
                warning.message
            )));
        }

        let mut input = Vec::new();
        input.push(if self.xkb {
            Row::new(Check::Ok, "XKB symbol compiler")
        } else {
            Row::new(Check::Fail, "XKB symbol compiler")
                .reason("install xkbcommon-tools for keycode translation")
        });
        if self.remappers.is_empty() {
            input.push(Row::new(Check::Skip, "remappers").reason("none detected"));
        } else {
            let mut row = Row::new(Check::Ok, "remappers").reason(
                self.remappers
                    .iter()
                    .map(|remapper| remapper.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            for remapper in self.remappers {
                for process in &remapper.processes {
                    row = row.note(format!("{}: process {process}", remapper.name));
                }
                for configuration in &remapper.configurations {
                    row = row.note(format!("{}: config {configuration}", remapper.name));
                }
                for transformation in &remapper.transformations {
                    row = row.note(format!(
                        "{}: static transformation {transformation}",
                        remapper.name
                    ));
                }
            }
            input.push(row.note("transformations and virtual-device routing remain conditional"));
        }
        if self.ime.is_empty() {
            input.push(Row::new(Check::Skip, "input method").reason("none detected"));
        } else {
            let mut row = Row::new(Check::Ok, "input method").reason(
                self.ime
                    .iter()
                    .map(|engine| engine.engine.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            for engine in self.ime {
                for source in &engine.sources {
                    row = row.note(format!("{}: source {source}", engine.engine));
                }
                for process in &engine.processes {
                    row = row.note(format!("{}: process {process}", engine.engine));
                }
                if let Some(active) = &engine.active_engine {
                    row = row.note(format!("{}: active engine {active}", engine.engine));
                }
                if let Some(state) = &engine.state {
                    row = row.note(format!("{}: runtime state {state}", engine.engine));
                }
                if let Some(error) = &engine.query_error {
                    row = row.note(format!(
                        "{}: runtime query unavailable: {error}",
                        engine.engine
                    ));
                }
            }
            input.push(row.note("committed text and Compose/dead-key state remain conditional"));
        }

        let mut shell = Vec::new();
        if self.generic_terminal_fallback {
            let mut row = Row::new(Check::Ok, "terminal").reason(format!(
                "{} (generic fallback; no Ghostty adapter needed)",
                self.terminal_program.unwrap_or("identity unknown")
            ));
            if let Some(adapter) = self.terminal_adapter {
                row = row.note(format!("terminal adapter: {adapter}"));
            }
            shell.push(row);
        } else {
            let row = Row::new(state(self.ghostty), "Ghostty keybinds");
            shell.push(if self.ghostty {
                row
            } else {
                row.reason("install Ghostty or use its config")
            });
        }
        let shell_name = self.shell.rsplit('/').next().unwrap_or_default();
        if self.shell.is_empty() {
            shell.push(Row::new(Check::Fail, "shell").reason("SHELL is not set"));
        } else {
            let mut row = Row::new(Check::Ok, shell_name).reason(self.shell);
            if self.ssh {
                row = row.note("SSH session: yes");
            }
            shell.push(row);
        }
        let init_shell = match shell_name {
            "bash" | "zsh" | "fish" => shell_name,
            _ => "<bash|zsh|fish>",
        };
        let snapshot = Row::new(state(self.shell_snapshot), "live shell bindings");
        shell.push(if self.shell_snapshot {
            snapshot
        } else {
            snapshot.fix(format!("eval \"$(whykey shell-init {init_shell})\""))
        });
        let multiplexer = match self.multiplexer_names {
            [] => None,
            ["tmux"] => Some("tmux detected (bindings inspected per key)"),
            ["screen"] => {
                Some("GNU Screen detected (prefix and screenrc bindings inspected per key)")
            }
            ["zellij"] => Some("Zellij detected (custom bindings inspected per key)"),
            _ => Some("nested multiplexers detected (each layer inspected independently)"),
        };
        shell.push(match multiplexer {
            Some(text) => Row::new(Check::Ok, "multiplexer").reason(text),
            None => Row::new(Check::Skip, "multiplexer").reason("none detected"),
        });
        shell.push(if self.nvim_extension {
            Row::new(Check::Ok, "Neovim extension").reason(match self.nvim_server {
                Some(server) => format!("whykey-nvim installed; server reachable ({server})"),
                None => "whykey-nvim installed; no server listening (start nvim with --listen)"
                    .to_owned(),
            })
        } else {
            Row::new(Check::Skip, "Neovim extension")
                .reason("whykey-nvim not found on PATH or extensions/")
        });

        let sources = self
            .layer_sources
            .iter()
            .map(|source| {
                Row::plain(source.layer).reason(format!(
                    "{} \u{2014} {}",
                    source.source.label(),
                    source.detail
                ))
            })
            .collect();

        vec![
            Group::new("Capture", capture),
            Group::new("Live readers", live),
            Group::new("Input stack", input),
            Group::new("Shell and editors", shell),
            Group::new("Where each layer's answer comes from", sources),
        ]
    }
}

/// What doctor learned that decides each layer's answer source.
struct DoctorLayerFacts<'a> {
    environment: &'a whykey::environment::Environment,
    x11_grab: bool,
    ghostty: bool,
    shell: &'a str,
    shell_snapshot: bool,
    nvim_server: Option<&'a str>,
}

/// Per layer: is its answer a live query of the running program, parsed
/// configuration, or nothing at all in this session?
fn doctor_layer_sources(facts: &DoctorLayerFacts<'_>) -> Vec<whykey::doctor::LayerSource> {
    use whykey::doctor::{LayerSource, SourceKind};
    let environment = facts.environment;
    let mut sources = Vec::new();
    sources.push(if environment.remappers.is_empty() {
        LayerSource::new("remapper", SourceKind::None, "no remapper detected")
    } else {
        LayerSource::new(
            "remapper",
            SourceKind::ConfigFile,
            environment
                .remappers
                .iter()
                .map(|remapper| remapper.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    });
    let desktop = whykey::registry::DESKTOPS
        .iter()
        .find(|entry| entry.doctor.is_some() && environment.desktop(entry.id).applicable);
    sources.push(match desktop {
        Some(entry) => {
            let reachable = environment.desktop(entry.id).ipc;
            let source = whykey::doctor::desktop_source(entry.id, reachable);
            let detail = if reachable {
                entry.display.to_owned()
            } else {
                format!("{} (not reachable)", entry.display)
            };
            LayerSource::new("compositor/desktop", source, detail)
        }
        None if environment.ssh => LayerSource::new(
            "compositor/desktop",
            SourceKind::None,
            "remote session; the local desktop is not inspected",
        ),
        None => LayerSource::new(
            "compositor/desktop",
            SourceKind::None,
            environment
                .compositor_context
                .desktop
                .as_deref()
                .map_or_else(
                    || "no supported desktop detected".to_owned(),
                    |desktop| format!("{desktop} has no read-only adapter"),
                ),
        ),
    });
    if facts.x11_grab {
        sources.push(LayerSource::new(
            "X11 key grabs",
            SourceKind::LiveQuery,
            "the X server reports which shortcuts a client has grabbed",
        ));
    }
    let live_ime = environment
        .ime
        .iter()
        .any(|engine| engine.state.is_some() || engine.active_engine.is_some());
    sources.push(match (environment.ime.is_empty(), live_ime) {
        (true, _) => LayerSource::new("input method", SourceKind::None, "none detected"),
        (false, true) => LayerSource::new(
            "input method",
            SourceKind::LiveQuery,
            "the input-method daemon reports its active engine",
        ),
        (false, false) => LayerSource::new(
            "input method",
            SourceKind::None,
            "detected, but its engine state cannot be queried",
        ),
    });
    sources.push(match environment.terminal {
        Some("Ghostty") if facts.ghostty => LayerSource::new(
            "terminal",
            SourceKind::LiveQuery,
            "Ghostty (`ghostty +list-keybinds`)",
        ),
        Some("WezTerm") => LayerSource::new(
            "terminal",
            SourceKind::LiveQuery,
            "WezTerm (`wezterm show-keys`)",
        ),
        Some(terminal) => LayerSource::new("terminal", SourceKind::ConfigFile, terminal),
        None => LayerSource::new(
            "terminal",
            SourceKind::None,
            "no terminal adapter; bytes are predicted generically",
        ),
    });
    let mut multiplexers = Vec::new();
    if environment.tmux {
        multiplexers.push(LayerSource::new(
            "multiplexer",
            SourceKind::LiveQuery,
            "tmux server (key tables)",
        ));
    }
    if environment.screen {
        multiplexers.push(LayerSource::new(
            "multiplexer",
            SourceKind::ConfigFile,
            "GNU Screen (.screenrc)",
        ));
    }
    if environment.zellij {
        multiplexers.push(LayerSource::new(
            "multiplexer",
            SourceKind::ConfigFile,
            "Zellij configuration",
        ));
    }
    if multiplexers.is_empty() {
        multiplexers.push(LayerSource::new(
            "multiplexer",
            SourceKind::None,
            "none detected",
        ));
    }
    sources.extend(multiplexers);
    let shell_name = facts.shell.rsplit('/').next().unwrap_or(facts.shell);
    sources.push(match (shell_name, facts.shell_snapshot) {
        ("bash" | "zsh" | "fish", true) => LayerSource::new(
            "shell",
            SourceKind::LiveQuery,
            format!("{shell_name} bindings dumped by shell-init"),
        ),
        ("bash" | "zsh" | "fish", false) => LayerSource::new(
            "shell",
            SourceKind::ConfigFile,
            format!(
                "{shell_name} defaults and startup files; load `whykey shell-init {shell_name}` for live bindings"
            ),
        ),
        _ => LayerSource::new("shell", SourceKind::None, "no supported shell"),
    });
    sources.push(match facts.nvim_server {
        Some(server) => LayerSource::new(
            "editor",
            SourceKind::LiveQuery,
            format!("Neovim RPC ({server})"),
        ),
        None => LayerSource::new("editor", SourceKind::None, "no Neovim server is listening"),
    });
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_vocabulary_is_shared() {
        let help = help_text();
        for command in COMMANDS {
            assert!(
                help.contains(&cli::usage(command.name)),
                "help is missing usage for {}",
                command.name
            );
            for shell in COMPLETION_SHELLS {
                let completion = completions(shell);
                assert!(
                    completion.contains(command.name),
                    "{shell} completions are missing {}",
                    command.name
                );
                for option in command.options().filter(|option| !option.alias) {
                    let long = option.long.trim_start_matches('-');
                    assert!(
                        completion.contains(option.long)
                            || completion.contains(&format!("-l {long}")),
                        "{shell} completions are missing {} for {}",
                        option.long,
                        command.name
                    );
                }
            }
        }
        let names: Vec<_> = COMMANDS.iter().map(|command| command.name).collect();
        assert!(names.contains(&"inspect"));
        assert!(names.contains(&"doctor"));
        assert!(help_text().contains("--suppress|--no-suppress|--pass-through"));
        for shell in COMPLETION_SHELLS {
            let completion = completions(shell);
            assert!(
                !completion.contains("--pass-through") && !completion.contains("-l pass-through"),
                "{shell} exposes hidden alias"
            );
        }
    }

    #[test]
    fn doctor_next_steps_are_specific_and_safe() {
        let steps = doctor_next_steps(&DoctorState {
            tty: false,
            compositor_context: false,
            ghostty: false,
            generic_terminal: false,
            xkb: false,
            evdev: whykey::doctor::EvdevScan::KeyboardDevices { readable: false },
            evdev_acl: None,
            shell_snapshot: false,
            ssh: false,
            remapper: true,
            ime: true,
        });
        assert!(
            steps
                .iter()
                .any(|step| step.contains("controlling terminal"))
        );
        assert!(steps.iter().any(|step| step.contains("xkbcommon-tools")));
        assert!(steps.iter().any(|step| step.contains("physical capture")));
        assert!(steps.iter().all(|step| !step.contains("reload")));
    }

    #[test]
    fn parses_inventory_filter_options_without_normalizing_values() {
        let filters = cli::parse_inventory_filters(
            vec![
                "--key".into(),
                "CTRL+X".into(),
                "--action".into(),
                "Open Terminal".into(),
                "--source".into(),
                "Hyprland".into(),
            ],
            "usage",
        )
        .unwrap();
        assert_eq!(filters.key.as_deref(), Some("CTRL+X"));
        assert_eq!(filters.action.as_deref(), Some("Open Terminal"));
        assert_eq!(filters.source.as_deref(), Some("Hyprland"));
    }
}
