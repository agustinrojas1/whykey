use std::env;
use std::process::ExitCode;

mod cli;

#[cfg(test)]
use cli::{COMMANDS, COMPLETION_SHELLS};
use cli::{
    DoctorState, command_program_available, command_succeeds, completions, detect_extension,
    detect_nvim_server, detected_terminal_program, doctor_next_steps, help_text,
    print_check_with_options, shell_init, shell_snapshot_available,
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
    let parsed = match cli::parse_global_arguments(env::args().skip(1).collect()) {
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
        schema_version,
    } = parsed;
    let mut arguments = raw_arguments.into_iter();
    let Some(argument) = arguments.next() else {
        eprintln!("{}", help_text());
        return ExitCode::from(2);
    };

    if matches!(argument.as_str(), "-h" | "--help") {
        println!("{}", help_text());
        return ExitCode::SUCCESS;
    }
    if matches!(argument.as_str(), "-V" | "--version") {
        println!("whykey {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    cli::dispatch(
        argument,
        arguments.collect(),
        json,
        ndjson,
        verbose,
        color,
        schema_version,
    )
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
            Ok::<_, String>(reports)
        })
    };
    let reports_result = match instance.as_deref() {
        Some(instance) => hyprland::with_instance_override(instance, inspect_with_budget),
        None => inspect_with_budget(),
    };
    let reports = match reports_result {
        Ok(reports) => reports,
        Err(error) => {
            eprintln!("whykey inspect --focused: {error}");
            return ExitCode::from(1);
        }
    };
    let has_explicit_target = target_pid.is_some() || focused;
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
    program: String,
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
        extensions::inspect(&program, &key)
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
    path: String,
    json: bool,
    verbose: bool,
    _color: ColorChoice,
    schema_version: u8,
) -> ExitCode {
    let documents = match replay::load(std::path::Path::new(&path)) {
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
    let unavailable = results
        .iter()
        .any(|result| result.outcome == Outcome::Unavailable);
    let value = snapshot::create(&key, &results);
    if let Some(path) = output_path {
        if let Err(error) = snapshot::save(std::path::Path::new(&path), &value) {
            eprintln!("whykey snapshot: could not write {path}: {error}");
            return ExitCode::from(1);
        }
        eprintln!("whykey snapshot: wrote {path}");
    } else {
        print!("{}", snapshot::render(&value));
    }
    if unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_diff(paths: Vec<String>, json: bool, _color: ColorChoice, schema_version: u8) -> ExitCode {
    let load_one = |path: &str| -> Result<serde_json::Value, String> {
        let documents =
            replay::load(std::path::Path::new(path)).map_err(|error| error.to_string())?;
        if documents.len() != 1 {
            return Err(format!(
                "{path} contains {} reports; expected exactly one",
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
    let native_compositor = whykey::capabilities::native_capture_available_for_doctor();
    let native_sway = whykey::sway_capture::probe_available();
    let native_available = native_compositor || native_sway;
    let native_backend = if native_compositor {
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
                        "IPC connection accepted"
                    } else {
                        "IPC connection unavailable"
                    },
                })
            })
        })
        .collect::<Vec<_>>();
    let native_cause = if native_available {
        whykey::doctor::Cause::Available
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
            .then(|| env::var("WAYLAND_DISPLAY").ok())
            .flatten(),
        x11: env::var("DISPLAY").ok().filter(|value| !value.is_empty()),
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
        let print_check = |name: &str, ok: bool, hint: &str| {
            print_check_with_options(name, ok, hint, render_options);
        };
        println!("{}\n", render_options.accent("whykey doctor"));
        print_check("controlling TTY", tty, "run inside a terminal");
        print_check(
            &format!("Native capture ({native_backend})"),
            native_available,
            "run inside a supported compositor session and ensure its IPC is reachable",
        );
        if !native_available {
            let cause = format!(
                "cause: {} — {}",
                native_cause.label(),
                native_cause.next_check()
            );
            for line in whykey::style::wrap_hanging(&cause, render_options.width, "  ", "  ") {
                println!("{}", render_options.uncertain(line));
            }
        }
        println!(
            "\n{}",
            render_options.accent("Live compositor runtime readers")
        );
        for reader in &runtime_readers {
            print_check(reader.id, reader.availability.available(), &reader.evidence);
        }
        if !native_available && !native_attempts.is_empty() {
            let attempted = native_attempts
                .iter()
                .filter_map(|entry| entry.get("backend").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{}",
                render_options.muted(format!(
                    "  attempted: {attempted} (unavailable: IPC connection unavailable)"
                ))
            );
        }
        if !environment.extension_adapters.is_empty() {
            println!(
                "\n{}",
                render_options.accent("Compositor extension adapters")
            );
            for adapter in &environment.extension_adapters {
                let applicable = whykey::extension_adapters::applicable_with_context(
                    adapter,
                    &environment.compositor_context,
                );
                let reachable = command_program_available(&adapter.bindings_cmd);
                print_check(
                    &format!("{} manifest", adapter.display),
                    applicable && reachable,
                    if !applicable {
                        "environment or desktop hints do not match"
                    } else if !reachable {
                        "bindings command executable was not found"
                    } else {
                        "manifest applies; commands are queried only when needed"
                    },
                );
            }
        }
        for warning in &environment.extension_warnings {
            let warning = format!(
                "FAIL  extension manifest: {} ({})",
                warning.path.display(),
                warning.message
            );
            for line in whykey::style::wrap_hanging(&warning, render_options.width, "", "  ") {
                println!("{}", render_options.failure(line));
            }
        }
        // The registry owns desktop detection, IPC status, check labels,
        // and hints; the first applicable desktop adapter wins.
        let selected = whykey::registry::DESKTOPS
            .iter()
            .find(|entry| entry.doctor.is_some() && environment.desktop(entry.id).applicable);
        if let Some(entry) = selected {
            let doctor = entry.doctor.expect("selected desktop has doctor metadata");
            let status = environment.desktop(entry.id);
            let ipc = status.applicable && status.ipc;
            if entry.id == "programmable" {
                let name = programmable::detect().map_or("programmable X11 WM", |wm| wm.name());
                print_check(name, ipc, doctor.hint);
            } else {
                print_check(doctor.check, ipc, doctor.hint);
            }
        } else if ssh {
            println!(
                "{}",
                render_options.success("OK  Hyprland IPC: not applicable in this SSH session")
            );
        } else if generic_compositor {
            let desktop = compositor_context
                .desktop
                .as_deref()
                .unwrap_or("unknown desktop");
            let session = compositor_context
                .session_type
                .as_deref()
                .or(compositor_context.display_server.as_deref())
                .unwrap_or("unknown display session");
            let message = format!(
                "FAIL  desktop compositor: {desktop} ({session}) detected, but no read-only adapter is available"
            );
            for line in whykey::style::wrap_hanging(&message, render_options.width, "", "  ") {
                println!("{}", render_options.failure(line));
            }
        } else {
            println!(
                "{}",
                render_options.failure("FAIL  compositor session: not detected")
            );
        }
        if generic_terminal_fallback {
            println!(
                "{}",
                render_options.success(format!(
                    "OK  terminal: {} (Ghostty adapter not required; generic fallback available)",
                    terminal_program.as_deref().unwrap_or("identity unknown")
                ))
            );
            if let Some(adapter) = terminal_adapter {
                println!(
                    "{}",
                    render_options.muted(format!("  terminal adapter: {adapter}"))
                );
            }
        } else {
            print_check(
                "Ghostty effective keybinds",
                ghostty,
                "install Ghostty or use its config",
            );
        }
        print_check(
            "XKB symbol compiler",
            xkb,
            "install xkbcommon-tools for keycode translation",
        );
        print_check(
            "evdev keyboard capture",
            evdev,
            evdev_next.as_deref().unwrap_or_default(),
        );
        if evdev_devices.is_empty() {
            println!(
                "{}",
                render_options.muted(format!("  evdev devices: none ({})", evdev_scan.detail()))
            );
        } else {
            println!("{}", render_options.muted("  evdev devices:"));
            for device in evdev_devices.iter().take(5) {
                let state = if device.readable {
                    "readable"
                } else {
                    device.error.as_deref().unwrap_or("not readable")
                };
                println!("    {} — {} ({state})", device.path, device.name);
            }
            if evdev_devices.len() > 5 {
                println!("    … {} more", evdev_devices.len() - 5);
            }
        }
        if remappers.is_empty() {
            println!("{}", render_options.muted("  remappers: none detected"));
        } else {
            println!("{}", render_options.muted("  remappers:"));
            for remapper in &remappers {
                println!("    {}", remapper.name);
                for process in &remapper.processes {
                    println!("      process: {process}");
                }
                for configuration in &remapper.configurations {
                    println!("      config: {configuration}");
                }
                for transformation in &remapper.transformations {
                    println!("      static transformation: {transformation}");
                }
            }
            println!(
                "{}",
                render_options
                    .uncertain("    transformations and virtual-device routing remain conditional")
            );
        }
        if ime.is_empty() {
            println!("{}", render_options.muted("  input method: none detected"));
        } else {
            println!("{}", render_options.muted("  input methods:"));
            for engine in &ime {
                println!("    {}", engine.engine);
                for source in &engine.sources {
                    println!("      source: {source}");
                }
                for process in &engine.processes {
                    println!("      process: {process}");
                }
                if let Some(active_engine) = &engine.active_engine {
                    println!("      active engine: {active_engine}");
                }
                if let Some(state) = &engine.state {
                    println!("      runtime state: {state}");
                }
                if let Some(error) = &engine.query_error {
                    println!("      runtime query unavailable: {error}");
                }
            }
            let message = "committed text and Compose/dead-key state remain conditional";
            for line in whykey::style::wrap_hanging(message, render_options.width, "    ", "    ") {
                println!("{}", render_options.uncertain(line));
            }
        }
        if shell.is_empty() {
            println!(
                "{}",
                render_options.failure("FAIL  shell: SHELL is not set")
            );
        } else {
            println!("{}", render_options.success(format!("OK  shell: {shell}")));
        }
        print_check(
            "shell runtime snapshot",
            shell_snapshot,
            "run `eval \"$(whykey shell-init <bash|zsh|fish>)\"` in the current shell",
        );
        let multiplexer_hint = match multiplexer_names.as_slice() {
            [] => "none detected",
            ["tmux"] => "tmux detected (bindings inspected per key)",
            ["screen"] => "GNU Screen detected (prefix and screenrc bindings inspected per key)",
            ["zellij"] => "Zellij detected (custom bindings inspected per key)",
            _ => "nested multiplexers detected (each layer inspected independently)",
        };
        println!(
            "{}",
            render_options.muted(format!("  multiplexer: {multiplexer_hint}"))
        );
        if ssh {
            println!("{}", render_options.muted("  SSH session: yes"));
        }
        if nvim_extension {
            println!(
                "{}",
                render_options.success("OK  Neovim extension: installed (whykey-nvim)")
            );
            if let Some(server) = &nvim_server {
                println!(
                    "{}",
                    render_options.muted(format!("  Neovim server: reachable ({server})"))
                );
            } else {
                println!(
                    "{}",
                    render_options
                        .muted("  Neovim server: none active (start nvim with --listen)",)
                );
            }
        } else {
            println!(
                "{}",
                render_options
                    .muted("Neovim extension: not found on PATH or extensions/ (whykey-nvim)")
            );
        }
        println!(
            "\n{}",
            render_options.accent("Observation taps (what whykey can watch now)")
        );
        for tap in &observation_taps {
            let line = format!(
                "{}  {}: {}",
                if tap.available { "OK" } else { "FAIL" },
                tap.name,
                tap.detail
            );
            for wrapped in whykey::style::wrap_hanging(&line, render_options.width, "", "  ") {
                if tap.available {
                    println!("{}", render_options.success(wrapped));
                } else {
                    println!("{}", render_options.failure(wrapped));
                }
            }
            if let Some(enable) = &tap.enable {
                for wrapped in
                    whykey::style::wrap_hanging(enable, render_options.width, "  enable: ", "    ")
                {
                    println!("{}", render_options.muted(wrapped));
                }
            }
        }
        println!(
            "\n{}",
            render_options.accent("Layer sources (live query, config file, or none)")
        );
        for source in &layer_sources {
            let line = format!(
                "{}: {} — {}",
                source.layer,
                source.source.label(),
                source.detail
            );
            for wrapped in whykey::style::wrap_hanging(&line, render_options.width, "  ", "    ") {
                println!("{wrapped}");
            }
        }
        if !next_steps.is_empty() {
            println!("\n{}", render_options.accent("Next steps"));
            for step in &next_steps {
                for line in whykey::style::wrap_hanging(step, render_options.width, "- ", "  ") {
                    println!("{line}");
                }
            }
        }
    }
    if tty && compositor_context_available && (ghostty || generic_terminal_fallback) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
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
