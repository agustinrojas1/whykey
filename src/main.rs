use std::env;
use std::process::{Command, ExitCode};
use std::time::Duration;

use whykey::bindings;
use whykey::capabilities;
use whykey::command;
use whykey::conflicts;
use whykey::diff;
use whykey::extensions;
use whykey::focus;
use whykey::key::{KeyCombo, KeySequence};
use whykey::layers::{
    LayerId, LayerResult, LayerStatus, Outcome, Propagation, hyprland, inspect_default_chain,
    inspect_default_chain_for_pid, inspect_default_chain_for_pid_with_source, programmable,
};
use whykey::listen;
use whykey::replay;
use whykey::report;
use whykey::schema;
use whykey::snapshot;

/// One source of truth for command vocabulary. Help text, the argument
/// parser below, and shell completions all derive from [`COMMANDS`]; the
/// `command_vocabulary_is_shared` test proves they agree.
struct CommandSpec {
    name: &'static str,
    usage: &'static str,
    summary: &'static str,
    advanced: bool,
}

#[derive(Default)]
struct InventoryFilters {
    key: Option<String>,
    action: Option<String>,
    source: Option<String>,
    device: Option<String>,
    submap: Option<String>,
}

struct DoctorState {
    tty: bool,
    compositor_context: bool,
    ghostty: bool,
    generic_terminal: bool,
    xkb: bool,
    evdev: bool,
    shell_snapshot: bool,
    ssh: bool,
    remapper: bool,
    ime: bool,
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
        usage: "whykey bindings [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--json] [--schema-version 2]",
        summary: "enumerate effective bindings",
        advanced: true,
    },
    CommandSpec {
        name: "conflicts",
        usage: "whykey conflicts [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--json] [--schema-version 2]",
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
        name: "snapshot",
        usage: "whykey snapshot <combination> [--output PATH]",
        summary: "save a static diagnostic snapshot without capturing input",
        advanced: true,
    },
    CommandSpec {
        name: "diff",
        usage: "whykey diff <before> <after> [--json]",
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
const SNAPSHOT_COMPLETION_OPTIONS: &[&str] = &["--output"];
const INVENTORY_FILTER_OPTIONS: &[&str] =
    &["--key", "--action", "--source", "--device", "--submap"];

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
        "\nExamples:\n  whykey ctrl+left\n  whykey ctrl+z\n  whykey super+c\n  whykey inspect ctrl+x ctrl+s\n\nReports show the conclusion first with only matching, consuming, unavailable, or uncertain layers. Add --verbose for the full evidence view; JSON keeps full structured evidence.\n\nwhykey does not edit configuration or execute shortcuts. Native compositor listen, currently backed by Hyprland, temporarily changes the compositor session and restores it when capture ends.",
    );
    output
}

fn parse_inventory_filters(
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
/// Downstream validation stays at the call site so messages never change.
fn take_value(
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
                        "Usage: whykey listen [--repeat] [--timeout SECONDS] [--count N] [--events all] [--pass-through] [--terminal] [--evdev] [--device PATH] [--verbose] [--json] [--ndjson] [--output PATH]\n\nCapture one key and explain its path. By default, uses native compositor capture (today: Hyprland) when available, temporarily suppressing shortcuts and falling back to the terminal otherwise. --pass-through captures through Hyprland without suppression; --terminal forces terminal capture; --evdev reads Linux keyboard events before the compositor without grabbing devices.\nEsc or Ctrl+C exits; --repeat captures another deliberate key after each report. --timeout is a wall-clock deadline; --count stops after N reports.\nUse --events all with native compositor or evdev capture to include modifier-only and release events. --verbose shows every route layer instead of only matching, consuming, unavailable, or uncertain layers. --json emits full analysis; --ndjson streams one compact record per event; --output writes reports to a file."
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
    if argument == "bindings" || argument == "conflicts" {
        let run: fn(bool, u8, InventoryFilters) -> ExitCode = if argument == "bindings" {
            run_bindings
        } else {
            run_conflicts
        };
        let usage = if argument == "bindings" {
            "whykey bindings [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--json] [--schema-version 2]"
        } else {
            "whykey conflicts [--key TEXT] [--action TEXT] [--source TEXT] [--device TEXT] [--submap TEXT] [--json] [--schema-version 2]"
        };
        let filters = match parse_inventory_filters(arguments.collect(), usage) {
            Ok(filters) => filters,
            Err(error) => {
                eprintln!("error: {error}");
                return ExitCode::from(2);
            }
        };
        return run(json, schema_version, filters);
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
    if argument == "snapshot" {
        return run_snapshot(arguments.collect());
    }
    if argument == "diff" {
        let paths: Vec<_> = arguments.collect();
        if paths.len() != 2 {
            eprintln!("error: usage is `whykey diff <before> <after> [--json]`");
            return ExitCode::from(2);
        }
        return run_diff(paths, json, schema_version);
    }
    if argument == "shell-init" || argument == "completions" {
        let print: fn(&str) -> String = if argument == "shell-init" {
            shell_init
        } else {
            completions
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
    let unavailable = inspection_failed(&results, false);
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

fn inspection_failed(layers: &[LayerResult], has_explicit_target: bool) -> bool {
    let target_unavailable = layers
        .iter()
        .any(|l| l.id == LayerId::Application && l.status() == LayerStatus::Unavailable);
    if has_explicit_target && target_unavailable {
        return true;
    }
    let budget_exceeded = layers
        .iter()
        .any(|l| l.id == LayerId::Diagnostic && l.status() == LayerStatus::Unavailable);
    if budget_exceeded {
        return true;
    }
    let has_handled = layers.iter().any(|l| {
        l.status() == LayerStatus::Handled
            || l.propagation() == Propagation::Stops
            || l.propagation() == Propagation::Redirected
    });
    if has_handled {
        return false;
    }
    !layers.is_empty()
        && layers
            .iter()
            .all(|l| l.status() == LayerStatus::Unavailable)
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
complete -c whykey -l pass-through -d 'capture without suppressing Hyprland shortcuts'
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
                let value = match take_value(&mut arguments, "--pid", "a positive process id") {
                    Ok(value) => value,
                    Err(code) => return code,
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
                let value = match take_value(&mut arguments, "--instance", "a Hyprland instance id")
                {
                    Ok(value) => value,
                    Err(code) => return code,
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
    let has_explicit_target = target_pid.is_some() || focused;
    let unavailable = reports
        .iter()
        .any(|steps| inspection_failed(steps, has_explicit_target));

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

fn run_bindings(json: bool, schema_version: u8, filters: InventoryFilters) -> ExitCode {
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
        print!("{}", bindings::render_text(&inventory));
    }
    if inventory.bindings.is_empty() && !inventory.unavailable.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn run_conflicts(json: bool, schema_version: u8, filters: InventoryFilters) -> ExitCode {
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

fn run_snapshot(arguments: Vec<String>) -> ExitCode {
    let mut arguments = arguments.into_iter();
    let Some(combination) = arguments.next() else {
        eprintln!("error: usage is `whykey snapshot <combination> [--output PATH]`");
        return ExitCode::from(2);
    };
    if combination == "-h" || combination == "--help" {
        println!("Usage: whykey snapshot <combination> [--output PATH]");
        println!("\nSave a static diagnostic snapshot. No key is captured or injected.");
        return ExitCode::SUCCESS;
    }
    let mut output_path = None;
    while let Some(argument) = arguments.next() {
        if argument != "--output" {
            eprintln!("error: usage is `whykey snapshot <combination> [--output PATH]`");
            return ExitCode::from(2);
        }
        let Some(path) = arguments.next() else {
            eprintln!("error: --output requires a path");
            return ExitCode::from(2);
        };
        if path.is_empty() {
            eprintln!("error: --output requires a non-empty path");
            return ExitCode::from(2);
        }
        output_path = Some(path);
    }

    let key: KeyCombo = match combination.parse() {
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

fn run_diff(paths: Vec<String>, json: bool, schema_version: u8) -> ExitCode {
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
        print!("{}", diff::render_text(&changes));
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
    let native_compositor = whykey::capabilities::native_capture_available_for_doctor();
    let native_backend = whykey::capture::NativeBackendId::Hyprland.display();
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
    let nvim_extension = detect_extension("whykey-nvim");
    let nvim_server = detect_nvim_server();

    let compositor_context_available = ssh
        || generic_compositor
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
        evdev,
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
            "native-compositor": {
                "available": native_compositor,
                "backend": native_compositor.then_some(native_backend)
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
        println!("whykey doctor\n");
        print_check("controlling TTY", tty, "run inside a terminal");
        print_check(
            &format!("Native capture ({native_backend})"),
            native_compositor,
            "set HYPRLAND_INSTANCE_SIGNATURE and ensure socket2 IPC is reachable",
        );
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
        if nvim_extension {
            println!("✓ Neovim extension: installed (whykey-nvim)");
            if let Some(server) = &nvim_server {
                println!("  Neovim server: reachable ({server})");
            } else {
                println!("  Neovim server: none active (start nvim with --listen)");
            }
        } else {
            println!("- Neovim extension: not found on PATH or extensions/ (whykey-nvim)");
        }
        if !next_steps.is_empty() {
            println!("\nNext steps:");
            for step in &next_steps {
                println!("- {step}");
            }
        }
    }
    if tty && compositor_context_available && (ghostty || generic_terminal_fallback) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn doctor_next_steps(state: &DoctorState) -> Vec<&'static str> {
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
        steps.push("compare remapper evidence with the physical capture; timing and routing remain conditional");
    }
    if state.ime {
        steps.push("check application-side Compose/dead-key or preedit state when text differs");
    }
    steps
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

fn detect_extension(name: &str) -> bool {
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            if dir.join(name).is_file() {
                return true;
            }
        }
    }
    std::path::Path::new("extensions").join(name).is_file()
}

fn detect_nvim_server() -> Option<String> {
    env::var("NVIM_LISTEN_ADDRESS")
        .ok()
        .or_else(|| env::var("NVIM").ok())
        .filter(|s| !s.is_empty())
        .or_else(scan_proc_nvim_server)
}

fn scan_proc_nvim_server() -> Option<String> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid_str = name.to_str()?;
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
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

    #[test]
    fn doctor_next_steps_are_specific_and_safe() {
        let steps = doctor_next_steps(&DoctorState {
            tty: false,
            compositor_context: false,
            ghostty: false,
            generic_terminal: false,
            xkb: false,
            evdev: false,
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
        let filters = parse_inventory_filters(
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
