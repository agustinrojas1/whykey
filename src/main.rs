use std::env;
use std::process::{Command, ExitCode};
use std::time::Duration;

use whykey::bindings;
use whykey::capabilities;
use whykey::command;
use whykey::conflicts;
use whykey::extensions;
use whykey::focus;
use whykey::key::{KeyCombo, KeySequence};
use whykey::layers::{
    Outcome, hyprland, inspect_default_chain, inspect_default_chain_for_pid,
    inspect_default_chain_for_pid_with_source, programmable,
};
use whykey::listen;
use whykey::replay;
use whykey::report;
use whykey::schema;

/// One source of truth for command vocabulary. Help text, the argument
/// parser below, and shell completions all derive from [`COMMANDS`]; the
/// `command_vocabulary_is_shared` test proves they agree.
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    advanced: bool,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "inspect",
        usage: "whykey inspect [--pid PID] [--focused] [--instance ID] [--verbose] [--json] [--schema-version 2] <combination-or-sequence>",
        summary: "explain one combination or sequence",
        advanced: false,
    },
    CommandSpec {
        name: "listen",
        usage: "whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--pass-through] [--terminal] [--evdev] [--device PATH] [--verbose] [--json] [--ndjson] [--output PATH] [--schema-version 2]",
        summary: "capture a key and explain its path",
        advanced: false,
    },
    CommandSpec {
        name: "doctor",
        usage: "whykey doctor [--json] [--schema-version 2]",
        summary: "check session integrations",
        advanced: false,
    },
    CommandSpec {
        name: "capabilities",
        usage: "whykey capabilities [--all] [--json] [--schema-version 2]",
        summary: "list applicable integrations (--all for the full inventory)",
        advanced: true,
    },
    CommandSpec {
        name: "bindings",
        usage: "whykey bindings [--json] [--schema-version 2]",
        summary: "enumerate effective bindings",
        advanced: true,
    },
    CommandSpec {
        name: "conflicts",
        usage: "whykey conflicts [--json] [--schema-version 2]",
        summary: "group duplicate actions",
        advanced: true,
    },
    CommandSpec {
        name: "extension",
        usage: "whykey extension <program> <combination> [--json] [--schema-version 2]",
        summary: "ask one opt-in external diagnostic",
        advanced: true,
    },
    CommandSpec {
        name: "replay",
        usage: "whykey replay <file> [--json]",
        summary: "re-render saved reports without injecting input",
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

const COMPLETION_SHELLS: &[&str] = &["bash", "zsh", "fish"];
const COMPLETION_EXAMPLES: &[&str] = &["ctrl+left", "ctrl+z", "super+c"];
const COMMON_COMPLETION_OPTIONS: &[&str] =
    &["--json", "--json-v2", "--schema-version", "--verbose"];
const INSPECT_COMPLETION_OPTIONS: &[&str] = &["--pid", "--focused", "--instance", "--verbose"];
const LISTEN_COMPLETION_OPTIONS: &[&str] = &[
    "--repeat",
    "--timeout",
    "--count",
    "--events",
    "--pass-through",
    "--terminal",
    "--evdev",
    "--device",
    "--ndjson",
    "--output",
    "--verbose",
];

/// Primary help leads with the common use; advanced commands follow.
/// Every usage line comes from [`COMMANDS`], so help, the parser, and
/// completions share one vocabulary.
fn help_text() -> String {
    let mut output =
        String::from("whykey - explain where a key combination is handled\n\nUsage:\n");
    output.push_str("  whykey [--verbose] [--json] [--schema-version 2] <combination>\n");
    for command in COMMANDS.iter().filter(|command| !command.advanced) {
        output.push_str(&format!("  {}\n    {}\n", command.usage, command.summary));
    }
    output.push_str("\nAdvanced:\n");
    for command in COMMANDS.iter().filter(|command| command.advanced) {
        output.push_str(&format!("  {}\n    {}\n", command.usage, command.summary));
    }
    output.push_str(
        "\nExamples:\n  whykey ctrl+left\n  whykey ctrl+z\n  whykey super+c\n  whykey inspect ctrl+x ctrl+s\n\nReports show the conclusion first with only matching, consuming, unavailable, or uncertain layers. Add --verbose for the full evidence view; JSON keeps full structured evidence.\n\nwhykey is read-only. It never changes configuration.",
    );
    output
}

fn main() -> ExitCode {
    let raw_arguments: Vec<_> = env::args().skip(1).collect();
    let json = raw_arguments
        .iter()
        .any(|argument| argument == "--json" || argument == "--json-v2" || argument == "--ndjson");
    let ndjson = raw_arguments.iter().any(|argument| argument == "--ndjson");
    let verbose = raw_arguments
        .iter()
        .any(|argument| argument == "--verbose" || argument == "-v");
    let mut schema_version = schema::DEFAULT_VERSION;
    let mut filtered_arguments = Vec::with_capacity(raw_arguments.len());
    let mut index = 0;
    while index < raw_arguments.len() {
        match raw_arguments[index].as_str() {
            "--json" | "--ndjson" => {}
            "--verbose" | "-v" => {}
            "--json-v2" => schema_version = 2,
            "--schema-version" => {
                let Some(value) = raw_arguments.get(index + 1) else {
                    eprintln!("error: --schema-version requires 1 or 2");
                    return ExitCode::from(2);
                };
                schema_version = match value.parse::<u8>() {
                    Ok(1 | 2) => value.parse().expect("validated schema version"),
                    _ => {
                        eprintln!("error: --schema-version must be 1 or 2");
                        return ExitCode::from(2);
                    }
                };
                index += 1;
            }
            argument => filtered_arguments.push(argument.to_owned()),
        }
        index += 1;
    }
    let raw_arguments = filtered_arguments;
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
    if ndjson && argument != "listen" {
        eprintln!("error: --ndjson is only supported by `whykey listen`");
        return ExitCode::from(2);
    }
    if argument == "inspect" {
        return run_inspect(arguments.collect(), json, verbose, schema_version);
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
        let mut capture_policy = listen::HyprlandCapturePolicy::Suppress;
        while let Some(option) = arguments.next() {
            match option.as_str() {
                "--repeat" | "-r" => repeat = true,
                "--pass-through" => capture_policy = listen::HyprlandCapturePolicy::PassThrough,
                "--terminal" | "-t" => terminal = true,
                "--evdev" | "-e" => evdev = true,
                "-h" | "--help" => {
                    println!(
                        "Usage: whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--pass-through] [--terminal] [--evdev] [--device PATH] [--verbose] [--json] [--ndjson] [--output PATH]\n\nCapture one key and explain its path. By default, captures and temporarily suppresses Hyprland shortcuts when available, falling back to the terminal. --pass-through captures via Hyprland without suppression; --terminal forces terminal capture; --evdev reads Linux keyboard events before the compositor without grabbing devices.\nEsc or Ctrl+C exits; --repeat captures another deliberate key after each report. --timeout is a wall-clock deadline; --count stops after N reports.\nUse --events all with Hyprland or evdev capture to include modifier-only and release events. --verbose shows every route layer instead of only matching, consuming, unavailable, or uncertain layers. --json emits full analysis; --ndjson streams one compact record per event; --output writes reports to a file."
                    );
                    return ExitCode::SUCCESS;
                }
                "--device" => {
                    let Some(path) = arguments.next() else {
                        eprintln!("error: --device requires a path");
                        return ExitCode::from(2);
                    };
                    device = Some(path.into());
                    evdev = true;
                }
                "--timeout" => {
                    let Some(value) = arguments.next() else {
                        eprintln!("error: --timeout requires seconds");
                        return ExitCode::from(2);
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
                    let Some(value) = arguments.next() else {
                        eprintln!("error: --count requires a positive integer");
                        return ExitCode::from(2);
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
                    let Some(value) = arguments.next() else {
                        eprintln!("error: --events requires 'all'");
                        return ExitCode::from(2);
                    };
                    if value != "all" {
                        eprintln!("error: unsupported event mode '{value}', expected 'all'");
                        return ExitCode::from(2);
                    }
                    events_all = true;
                }
                "--output" => {
                    let Some(path) = arguments.next() else {
                        eprintln!("error: --output requires a path");
                        return ExitCode::from(2);
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
        return run_listen(listen::Options {
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
            schema_version,
            capture_policy,
        });
    }
    if argument == "doctor" {
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey doctor [--json]`");
            return ExitCode::from(2);
        }
        return run_doctor(json, schema_version);
    }
    if argument == "capabilities" {
        let mut all = false;
        for option in arguments.by_ref() {
            if option == "--all" {
                all = true;
            } else {
                eprintln!("error: usage is `whykey capabilities [--all] [--json]`");
                return ExitCode::from(2);
            }
        }
        return run_capabilities(json, all, schema_version);
    }
    if argument == "bindings" {
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey bindings [--json]`");
            return ExitCode::from(2);
        }
        return run_bindings(json, schema_version);
    }
    if argument == "conflicts" {
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey conflicts [--json]`");
            return ExitCode::from(2);
        }
        return run_conflicts(json, schema_version);
    }
    if argument == "extension" {
        let Some(program) = arguments.next() else {
            eprintln!("error: usage is `whykey extension <program> <combination> [--json]`");
            return ExitCode::from(2);
        };
        let combinations: Vec<_> = arguments.collect();
        if combinations.is_empty() {
            eprintln!("error: usage is `whykey extension <program> <combination> [--json]`");
            return ExitCode::from(2);
        }
        return run_extension(program, combinations.join(" "), json, schema_version);
    }
    if argument == "replay" {
        let Some(path) = arguments.next() else {
            eprintln!("error: usage is `whykey replay <file> [--json]`");
            return ExitCode::from(2);
        };
        if arguments.next().is_some() {
            eprintln!("error: usage is `whykey replay <file> [--json]`");
            return ExitCode::from(2);
        }
        return run_replay(path, json, schema_version);
    }
    if argument == "shell-init" {
        match arguments.next().as_deref() {
            Some(shell @ ("bash" | "zsh" | "fish")) if arguments.next().is_none() => {
                println!("{}", shell_init(shell));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("error: usage is `whykey shell-init <bash|zsh|fish>`");
                return ExitCode::from(2);
            }
        }
    }
    if argument == "completions" {
        match arguments.next().as_deref() {
            Some(shell @ ("bash" | "zsh" | "fish")) if arguments.next().is_none() => {
                println!("{}", completions(shell));
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("error: usage is `whykey completions <bash|zsh|fish>`");
                return ExitCode::from(2);
            }
        }
    }
    if arguments.next().is_some() {
        eprintln!("error: expected one key combination\n\n{}", help_text());
        return ExitCode::from(2);
    }

    let key: KeyCombo = match argument.parse() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("error: {error}\n\nTry 'whykey --help'.");
            return ExitCode::from(2);
        }
    };

    let results = command::with_deadline(command::configured_diagnostic_timeout(), || {
        inspect_default_chain(&key)
    });
    let unavailable = results
        .iter()
        .any(|result| result.outcome == Outcome::Unavailable);
    if json {
        print!(
            "{}",
            report::render_json(&key, &results, None, schema_version)
        );
    } else {
        print!("{}", report::render(&key, &results, verbose));
    }

    if unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn shell_init(shell: &str) -> String {
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

fn completions(shell: &str) -> String {
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
  elif [[ "${COMP_WORDS[1]}" == "doctor" || "${COMP_WORDS[1]}" == "bindings" || "${COMP_WORDS[1]}" == "conflicts" ]]; then
    COMPREPLY=( $(compgen -W "__COMMON_OPTIONS__" -- "$cur") )
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
    '--all[list the complete adapter inventory]' \
    '--pid[inspect process ancestry]:pid:' \
    '--focused[inspect focused window]' \
    '--instance[select a Hyprland instance]:instance:' \
    '--repeat[keep listening]' \
    '--timeout[stop after this many seconds]:seconds:' \
    '--count[stop after this many reports]:count:' \
    '--events[include modifier and release events]:mode:(all)' \
    '--pass-through[capture without suppressing Hyprland shortcuts]' \
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
complete -c whykey -n '__fish_seen_subcommand_from capabilities' -l all -d 'list the complete adapter inventory'
complete -c whykey -l pid -r -d 'inspect the process ancestry rooted at this PID'
complete -c whykey -l focused -d 'inspect the focused window process'
complete -c whykey -l instance -r -d 'select a Hyprland instance explicitly'
complete -c whykey -s r -l repeat -d 'keep listening'
complete -c whykey -l timeout -r -d 'stop after this many seconds'
complete -c whykey -l count -r -d 'stop after this many reports'
complete -c whykey -l events -r -a 'all' -d 'include modifier and release events'
complete -c whykey -l pass-through -d 'capture without suppressing Hyprland shortcuts'
complete -c whykey -s t -l terminal -d 'force terminal capture'
complete -c whykey -s e -l evdev -d 'capture Linux input events before the compositor'
complete -c whykey -l device -r -d 'read one /dev/input/event device'
complete -c whykey -n '__fish_seen_subcommand_from listen' -l ndjson -d 'emit one compact schema-v2 JSON record per captured event'
complete -c whykey -n '__fish_seen_subcommand_from listen' -l output -r -d 'write captured JSON reports to a file'
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
    ] {
        script = script.replace(placeholder, &value);
    }
    script
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

fn run_inspect(arguments: Vec<String>, json: bool, verbose: bool, schema_version: u8) -> ExitCode {
    let mut combinations = Vec::new();
    let mut target_pid = None;
    let mut focused = false;
    let mut instance = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--pid" => {
                let Some(value) = arguments.next() else {
                    eprintln!("error: --pid requires a positive process id");
                    return ExitCode::from(2);
                };
                let pid = match value.parse::<u32>() {
                    Ok(pid) if pid > 0 => pid,
                    _ => {
                        eprintln!("error: --pid must be a positive process id");
                        return ExitCode::from(2);
                    }
                };
                target_pid = Some(pid);
            }
            "--focused" => focused = true,
            "--instance" => {
                let Some(value) = arguments.next() else {
                    eprintln!("error: --instance requires a Hyprland instance id");
                    return ExitCode::from(2);
                };
                if value.trim().is_empty() {
                    eprintln!("error: --instance requires a non-empty Hyprland instance id");
                    return ExitCode::from(2);
                }
                instance = Some(value);
            }
            _ if argument.starts_with('-') => {
                eprintln!("error: unknown inspect option '{argument}'");
                return ExitCode::from(2);
            }
            _ => combinations.push(argument),
        }
    }

    if combinations.is_empty() {
        eprintln!(
            "error: usage is `whykey inspect <combination-or-sequence>`\n\n{}",
            help_text()
        );
        return ExitCode::from(2);
    }

    if focused && target_pid.is_some() {
        eprintln!("error: --pid and --focused cannot be used together");
        return ExitCode::from(2);
    }

    let input = combinations.join(" ");
    let sequence: KeySequence = match input.parse() {
        Ok(sequence) => sequence,
        Err(error) => {
            eprintln!("error: {error}\n\nTry 'whykey --help'.");
            return ExitCode::from(2);
        }
    };

    let inspect_with_budget = || {
        command::with_deadline(command::configured_diagnostic_timeout(), || {
            let focused_target = focused.then(focus::resolve).transpose()?;
            let resolved_pid = focused_target
                .as_ref()
                .map(|target| target.pid)
                .or(target_pid);
            let resolved_source = focused_target.as_ref().map(|target| target.source);
            let reports: Vec<_> = sequence
                .as_slice()
                .iter()
                .map(|key| {
                    resolved_pid.map_or_else(
                        || inspect_default_chain(key),
                        |pid| match resolved_source {
                            Some(source) => {
                                inspect_default_chain_for_pid_with_source(key, pid, Some(source))
                            }
                            None => inspect_default_chain_for_pid(key, pid),
                        },
                    )
                })
                .collect();
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
    let unavailable = reports
        .iter()
        .flatten()
        .any(|result| result.outcome == Outcome::Unavailable);

    if json {
        print!(
            "{}",
            report::render_sequence_json(&sequence, &reports, schema_version)
        );
    } else {
        print!("{}", report::render_sequence(&sequence, &reports, verbose));
    }

    if unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_capabilities(json: bool, all: bool, schema_version: u8) -> ExitCode {
    let capabilities = command::with_deadline(command::configured_diagnostic_timeout(), || {
        capabilities::current()
    });
    if json {
        print!(
            "{}",
            capabilities::render_json(&capabilities, schema_version)
        );
    } else {
        print!("{}", capabilities::render_text(&capabilities, all));
    }
    ExitCode::SUCCESS
}

fn run_bindings(json: bool, schema_version: u8) -> ExitCode {
    let inventory = command::with_deadline(command::configured_diagnostic_timeout(), || {
        bindings::current()
    });
    if json {
        print!("{}", bindings::render_json(&inventory, schema_version));
    } else {
        print!("{}", bindings::render_text(&inventory));
    }
    if inventory.bindings.is_empty() && !inventory.unavailable.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_conflicts(json: bool, schema_version: u8) -> ExitCode {
    let report = command::with_deadline(command::configured_diagnostic_timeout(), || {
        conflicts::current()
    });
    if json {
        print!("{}", conflicts::render_json(&report, schema_version));
    } else {
        print!("{}", conflicts::render_text(&report));
    }
    if report.conflicts.is_empty() && !report.unavailable.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_extension(program: String, input: String, json: bool, schema_version: u8) -> ExitCode {
    let key: KeyCombo = match input.parse() {
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
        print!("{}", extensions::render_text(&result, &key));
    }
    if result.layer.outcome == Outcome::Unavailable {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_replay(path: String, json: bool, schema_version: u8) -> ExitCode {
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
        print!("{}", replay::render_text(&documents));
    }
    ExitCode::SUCCESS
}

fn run_doctor(json: bool, schema_version: u8) -> ExitCode {
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
    let shell = env::var_os("SHELL")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let shell_snapshot = shell_snapshot_available(&shell);
    let evdev = listen::evdev_available();
    let evdev_devices = listen::evdev_devices();
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
            "evdev": {"available": evdev, "devices": evdev_devices},
            "remappers": remappers,
            "ime": ime,
            "terminal_program": terminal_program,
            "terminal_adapter": terminal_adapter,
            "shell": shell,
            "shell_snapshot": shell_snapshot,
            "multiplexer": multiplexer_display,
            "ssh": ssh,
        });
        legacy_value
            .as_object_mut()
            .expect("doctor checks are an object")
            .extend(desktop_checks);
        let value = if schema_version == 2 {
            serde_json::json!({
                "schema_version": 2,
                "operation": "doctor",
                "context": schema::context(),
                "checks": legacy_value,
            })
        } else {
            legacy_value
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&value).expect("doctor is serializable")
        );
    } else {
        println!("whykey doctor\n");
        print_check("controlling TTY", tty, "run inside a terminal");
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
            println!("✓ Hyprland IPC: not applicable in this SSH session");
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
            println!(
                "! desktop compositor: {desktop} ({session}) detected, but no read-only adapter is available"
            );
        } else {
            println!("! compositor session: not detected");
        }
        if generic_terminal_fallback {
            println!(
                "✓ terminal: {} (Ghostty adapter not required; generic fallback available)",
                terminal_program.as_deref().unwrap_or("identity unknown")
            );
            if let Some(adapter) = terminal_adapter {
                println!("  terminal adapter: {adapter}");
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
            "grant read access to /dev/input/event* (usually the input group)",
        );
        if evdev_devices.is_empty() {
            println!("  evdev devices: none detected");
        } else {
            println!("  evdev devices:");
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
            println!("  remappers: none detected");
        } else {
            println!("  remappers:");
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
            println!("    transformations and virtual-device routing remain conditional");
        }
        if ime.is_empty() {
            println!("  input method: none detected");
        } else {
            println!("  input methods:");
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
            println!("    committed text and Compose/dead-key state remain conditional");
        }
        if shell.is_empty() {
            println!("! shell: SHELL is not set");
        } else {
            println!("✓ shell: {shell}");
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
        println!("  multiplexer: {multiplexer_hint}");
        if ssh {
            println!("  SSH session: yes");
        }
    }

    let compositor_context_available = ssh
        || generic_compositor
        || whykey::registry::DESKTOPS.iter().any(|entry| {
            entry.doctor.is_some() && {
                let status = environment.desktop(entry.id);
                status.applicable && status.ipc
            }
        });
    if tty && compositor_context_available && (ghostty || generic_terminal_fallback) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn shell_snapshot_available(shell: &str) -> bool {
    match shell.rsplit('/').next().unwrap_or(shell) {
        "bash" => env::var_os("WHYKEY_READLINE_BINDINGS").is_some(),
        "zsh" => env::var_os("WHYKEY_ZLE_BINDINGS").is_some(),
        "fish" => env::var_os("WHYKEY_FISH_BINDINGS").is_some(),
        _ => false,
    }
}

fn command_succeeds(program: &str, arguments: &[&str]) -> bool {
    let mut command = Command::new(program);
    command.args(arguments);
    command::output(&mut command)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn detected_terminal_program() -> Option<String> {
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

fn print_check(name: &str, ok: bool, hint: &str) {
    if ok {
        println!("✓ {name}");
    } else {
        println!("! {name} ({hint})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_vocabulary_is_shared() {
        let help = help_text();
        for command in COMMANDS {
            assert!(
                help.contains(command.usage),
                "help is missing usage for {}",
                command.name
            );
            for shell in COMPLETION_SHELLS {
                assert!(
                    completions(shell).contains(command.name),
                    "{shell} completions are missing {}",
                    command.name
                );
            }
        }
        let names: Vec<_> = COMMANDS.iter().map(|command| command.name).collect();
        assert!(names.contains(&"inspect"));
        assert!(names.contains(&"doctor"));
    }
}
