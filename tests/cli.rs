use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::process::Stdio;

fn binary() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_whykey"));
    // CLI fixtures must not inherit the developer's live compositor, bus, or
    // terminal identity. Individual tests opt into the context they model.
    for variable in [
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "I3SOCK",
        "SSH_CONNECTION",
        "SSH_TTY",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "XDG_SESSION_TYPE",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "GTK_IM_MODULE",
        "QT_IM_MODULE",
        "XMODIFIERS",
        "TERM_PROGRAM",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        command.env_remove(variable);
    }
    command
}

fn temp_dir(label: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("whykey-{label}-{}-{id}", std::process::id()))
}

fn write_mock_bin(base: &Path, name: &str, content: &str) -> PathBuf {
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join(name);
    fs::write(&script, content).unwrap();
    let mut permissions = fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).unwrap();
    bin
}

fn bin_path(bin: &Path) -> String {
    let current = std::env::var_os("PATH").unwrap_or_default();
    format!("{}:{}", bin.display(), current.to_string_lossy())
}

#[test]
fn json_output_is_parseable_without_a_controlling_tty() {
    let output = binary().args(["--json", "ctrl+z"]).output().unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key"]["key"], "Z");
    assert_eq!(value["key_display"], "CTRL + Z");
    assert!(
        value["layers"]
            .as_array()
            .is_some_and(|layers| !layers.is_empty())
    );
}

#[test]
fn json_v2_exposes_context_and_structured_evidence() {
    let output = binary()
        .args(["--json", "--schema-version", "2", "ctrl+z"])
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["operation"], "inspect");
    assert_eq!(value["input"]["key_display"], "CTRL + Z");
    assert!(value["context"].is_object());
    assert!(
        value["path"]
            .as_array()
            .is_some_and(|path| { path.iter().any(|layer| layer["evidence"].is_array()) })
    );
}

#[test]
fn json_v2_is_available_for_capabilities() {
    let output = binary()
        .args(["capabilities", "--json", "--schema-version", "2"])
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["operation"], "capabilities");
    assert!(value["capabilities"].is_array());
}

#[test]
fn inspect_sequence_json_is_parseable() {
    let output = binary()
        .args(["inspect", "ctrl+x", "ctrl+s", "--json"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["sequence_display"], "CTRL+X CTRL+S");
    assert_eq!(value["steps"].as_array().unwrap().len(), 2);
    assert_eq!(value["steps"][0]["key_display"], "CTRL + X");
    assert!(value["steps"][0]["layers"].is_array());
}

#[test]
fn inspect_sequence_text_reports_each_step() {
    let output = binary()
        .args(["inspect", "ctrl+x", "ctrl+s"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Sequence: CTRL+X CTRL+S"));
    assert!(text.contains("Step 1/2: CTRL + X"));
    assert!(text.contains("Step 2/2: CTRL + S"));
}

#[test]
fn inspect_rejects_an_invalid_sequence() {
    let output = binary()
        .args(["inspect", "ctrl+x", "ctrl+shift"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("step 2"));
}

#[test]
fn inspect_reports_when_focused_window_is_unavailable() {
    let output = binary()
        .args(["inspect", "ctrl+x", "--focused"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("focused-window"));
}

#[cfg(unix)]
#[test]
fn inspect_focused_uses_a_sway_tree_pid() {
    let base = temp_dir("focused-sway");
    let bin = write_mock_bin(
        &base,
        "swaymsg",
        "#!/bin/sh\ncase \"$*\" in *get_tree*) printf '%s\\n' '{\"nodes\":[{\"focused\":true,\"pid\":1}]}' ;; *) exit 1 ;; esac\n",
    );

    let output = binary()
        .args(["inspect", "ctrl+x", "--focused", "--verbose"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("SWAYSOCK", "/tmp/whykey-sway.sock")
        .env("XDG_CURRENT_DESKTOP", "sway")
        .env("XDG_SESSION_TYPE", "wayland")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("target pid: 1"));
    assert!(text.contains("target selection source: Sway get_tree"));
    assert!(text.contains("Interactive application"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_focused_uses_an_i3_tree_pid() {
    let base = temp_dir("focused-i3");
    let bin = write_mock_bin(
        &base,
        "i3-msg",
        "#!/bin/sh\ncase \"$*\" in *get_tree*) printf '%s\\n' '{\"nodes\":[{\"focused\":true,\"pid\":1}]}' ;; *) exit 1 ;; esac\n",
    );

    let output = binary()
        .args(["inspect", "ctrl+x", "--focused", "--verbose"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("I3SOCK", "/tmp/whykey-i3.sock")
        .env("XDG_CURRENT_DESKTOP", "i3")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("target pid: 1"));
    assert!(text.contains("target selection source: i3 get_tree"));
    assert!(text.contains("Interactive application"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_focused_uses_a_hyprland_activewindow_pid() {
    let base = temp_dir("focused-hyprland");
    let bin = write_mock_bin(
        &base,
        "hyprctl",
        "#!/bin/sh\ncase \"$*\" in *activewindow*) printf '%s\\n' '{\"pid\":1}' ;; *binds*) printf '%s\\n' '[]' ;; *submap*) printf '%s\\n' '\"default\"' ;; *devices*) printf '%s\\n' '{\"keyboards\":[]}' ;; *) exit 2 ;; esac\n",
    );
    write_mock_bin(&base, "ghostty", "#!/bin/sh\nexit 0\n");

    let output = binary()
        .args(["inspect", "ctrl+x", "--focused", "--verbose"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("HYPRLAND_INSTANCE_SIGNATURE", "test")
        .env("XDG_CURRENT_DESKTOP", "Hyprland")
        .env("XDG_SESSION_TYPE", "wayland")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("target pid: 1"));
    assert!(text.contains("target selection source: Hyprland activewindow"));
    assert!(text.contains("Interactive application"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn inspect_rejects_conflicting_target_selectors() {
    let output = binary()
        .args(["inspect", "ctrl+x", "--pid", "1", "--focused"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used together"));
}

#[cfg(unix)]
#[test]
fn inspect_accepts_an_explicit_hyprland_instance() {
    let base = temp_dir("hyprland-instance");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let hyprctl = bin.join("hyprctl");
    fs::write(
        &hyprctl,
        "#!/bin/sh\n[ \"$1\" = \"-i\" ] || exit 2\ninstance=$2\nshift 2\ncase \"$*\" in *binds*) printf '%s\\n' '[]' ;; *submap*) printf '%s\\n' '\"default\"' ;; *devices*) printf '%s\\n' '{\"keyboards\":[]}' ;; *) exit 2 ;; esac\n",
    )
    .unwrap();
    let ghostty = bin.join("ghostty");
    fs::write(&ghostty, "#!/bin/sh\nexit 0\n").unwrap();
    for command in [&hyprctl, &ghostty] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let output = binary()
        .args(["inspect", "ctrl+x", "--instance", "second", "--verbose"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("TERM_PROGRAM", "ghostty")
        .env("XDG_CURRENT_DESKTOP", "Hyprland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("selected Hyprland instance: second"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_discovers_a_valid_instance_beside_a_malformed_inventory_entry() {
    let base = temp_dir("hyprland-instance-discovery");
    let bin = base.join("bin");
    let log = base.join("hyprctl.log");
    fs::create_dir_all(&bin).unwrap();
    let hyprctl = bin.join("hyprctl");
    fs::write(
        &hyprctl,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HYPRCTL_LOG\"\nif [ \"$1\" = instances ]; then printf '%s\\n' '[{\"instance\":\"first\",\"wl_socket\":\"wayland-1\"},{\"instance\":42,\"wl_socket\":\"wayland-invalid\"},{\"instance\":\"second\",\"wl_socket\":\"wayland-2\"}]'; exit 0; fi\n[ \"$1\" = -i ] || exit 2\n[ \"$2\" = second ] || exit 2\nshift 2\ncase \"$*\" in *binds*) printf '%s\\n' '[]' ;; *submap*) printf '%s\\n' '\"default\"' ;; *devices*) printf '%s\\n' '{\"keyboards\":[]}' ;; *) exit 2 ;; esac\n",
    )
    .unwrap();
    let ghostty = bin.join("ghostty");
    fs::write(&ghostty, "#!/bin/sh\nexit 0\n").unwrap();
    for command in [&hyprctl, &ghostty] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let output = binary()
        .args(["inspect", "ctrl+x"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("HYPRCTL_LOG", log.to_string_lossy().into_owned())
        .env("WAYLAND_DISPLAY", "wayland-2")
        .env("XDG_CURRENT_DESKTOP", "Hyprland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .output()
        .unwrap();

    assert_ne!(
        output.status.code(),
        Some(2),
        "inspection output: {output:?}"
    );
    let calls = fs::read_to_string(log).unwrap();
    assert_eq!(
        calls.lines().count(),
        4,
        "unexpected hyprctl calls: {calls}"
    );
    assert_eq!(
        calls.lines().filter(|call| *call == "instances -j").count(),
        1,
        "instance discovery should be reused: {calls}"
    );
    assert!(calls.lines().any(|call| call == "-i second binds -j"));
    assert!(calls.lines().any(|call| call == "-i second submap -j"));
    assert!(calls.lines().any(|call| call == "-i second devices -j"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn inspect_accepts_a_process_target() {
    let pid = std::process::id().to_string();
    let output = binary()
        .args(["inspect", "ctrl+x", "--pid", "--verbose"])
        .arg(pid.clone())
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(&format!("target pid: {pid}")));
}

#[test]
fn inspect_reports_a_missing_process_target() {
    let output = binary()
        .args(["inspect", "--json", "ctrl+x", "--pid", "4294967295"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["steps"][0]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|layer| layer["layer"] == "Interactive application")
            .and_then(|layer| layer["status"].as_str()),
        Some("Unavailable")
    );
}

#[test]
fn invalid_combinations_have_a_cli_error_status() {
    let output = binary().arg("ctrl+shift").output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("non-modifier key"));
}

#[cfg(unix)]
#[test]
fn whykey_nvim_extension_returns_valid_schema_v1() {
    let output = binary()
        .args(["extension", "extensions/whykey-nvim", "ctrl+x", "--json"])
        .output()
        .unwrap();

    // Without a running nvim server, the extension returns Unavailable or Indeterminate (not CLI error 2).
    assert_ne!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key_display"], "CTRL + X");
    assert_eq!(value["extension"]["schema_version"], 1);
    assert!(
        value["extension"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "query_mode")
    );
}

#[cfg(unix)]
#[test]
fn whykey_nvim_extension_matches_fixture_runtime_queries() {
    let fixture_bin = std::fs::canonicalize("tests/fixtures/extensions/nvim/bin").unwrap();
    let path_env = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for (key, status, prop, needle) in [
        ("ctrl+x", "Handled", "Stops", ":echo 1<CR>"),
        ("ctrl+left", "Handled", "Stops", "copilot#Accept()"),
        ("ctrl+q", "NotHandled", "Continues", "no mapping"),
    ] {
        let output = binary()
            .args(["extension", "extensions/whykey-nvim", key, "--json"])
            .env("PATH", &path_env)
            .env("NVIM_LISTEN_ADDRESS", "/tmp/fake-nvim.sock")
            .output()
            .unwrap();
        assert!(output.status.success(), "case: {key}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["extension"]["layer"]["status"], status, "case: {key}");
        assert_eq!(
            value["extension"]["layer"]["propagation"], prop,
            "case: {key}"
        );
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains(needle), "case: {key}");
    }
}

#[cfg(unix)]
#[test]
fn whykey_vscode_extension_returns_valid_schema_v1() {
    let output = binary()
        .args(["extension", "extensions/whykey-vscode", "ctrl+x", "--json"])
        .env("HOME", "/nonexistent")
        .output()
        .unwrap();

    // Without a keybindings.json, extension returns Unavailable (exit 1), not CLI error 2.
    assert_ne!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key_display"], "CTRL + X");
    assert_eq!(value["extension"]["schema_version"], 1);
    assert_eq!(value["extension"]["capabilities"][0], "identify_action");
    assert_eq!(value["extension"]["layer"]["status"], "Unavailable");
}

#[cfg(unix)]
#[test]
fn whykey_vscode_extension_matches_fixture_keybindings() {
    let fixture_home = std::fs::canonicalize("tests/fixtures/extensions/vscode/home").unwrap();

    for (key, status, prop, needle) in [
        ("ctrl+x", "Handled", "Stops", "test.exact"),
        ("ctrl+y", "Handled", "Indeterminate", "not evaluated"),
        ("ctrl+k", "Handled", "Continues", "chord"),
        (
            "ctrl+q",
            "NotHandled",
            "Continues",
            "defaults not evaluated",
        ),
        (
            "ctrl+z",
            "NotHandled",
            "Continues",
            "defaults not evaluated",
        ),
    ] {
        let output = binary()
            .args(["extension", "extensions/whykey-vscode", key, "--json"])
            .env("HOME", &fixture_home)
            .output()
            .unwrap();
        assert!(output.status.success(), "case: {key}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["extension"]["layer"]["status"], status, "case: {key}");
        assert_eq!(
            value["extension"]["layer"]["propagation"], prop,
            "case: {key}"
        );
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains(needle), "case: {key}");
    }
}

#[cfg(unix)]
#[test]
fn whykey_emacs_extension_returns_valid_schema_v1() {
    let output = binary()
        .args(["extension", "extensions/whykey-emacs", "ctrl+x", "--json"])
        .env("EMACS_SERVER_FILE", "/nonexistent/emacs/server")
        .output()
        .unwrap();

    // Without a reachable emacs daemon, extension returns Unavailable (exit 1), not CLI error 2.
    assert_ne!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key_display"], "CTRL + X");
    assert_eq!(value["extension"]["schema_version"], 1);
    assert!(
        value["extension"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "query_mode")
    );
    assert_eq!(value["extension"]["layer"]["status"], "Unavailable");
}

#[cfg(unix)]
#[test]
fn whykey_emacs_extension_matches_fixture_runtime_queries() {
    let fixture_bin = std::fs::canonicalize("tests/fixtures/extensions/emacs/bin").unwrap();
    let path_env = format!(
        "{}:{}",
        fixture_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    for (key, status, prop, needle, winning_map) in [
        (
            "ctrl+x",
            "Handled",
            "Stops",
            "Control-X-prefix",
            Some("global"),
        ),
        (
            "ctrl+left",
            "Handled",
            "Stops",
            "org-left-click",
            Some("major"),
        ),
        (
            "ctrl+alt+return",
            "Handled",
            "Stops",
            "my-custom-minor-cmd",
            Some("minor"),
        ),
        ("ctrl+q", "NotHandled", "Continues", "no mapping", None),
    ] {
        let output = binary()
            .args(["extension", "extensions/whykey-emacs", key, "--json"])
            .env("PATH", &path_env)
            .output()
            .unwrap();
        assert!(output.status.success(), "case: {key}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["extension"]["layer"]["status"], status, "case: {key}");
        assert_eq!(
            value["extension"]["layer"]["propagation"], prop,
            "case: {key}"
        );
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        assert!(stdout_str.contains(needle), "case: {key}");
        if let Some(map) = winning_map {
            assert!(
                stdout_str.contains(&format!("winning-map: {map}")),
                "case: {key}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn extension_command_uses_the_versioned_stdin_stdout_protocol() {
    let base = temp_dir("extension");
    fs::create_dir_all(&base).unwrap();
    let extension = base.join("inspect-key");
    fs::write(
        &extension,
        r##"#!/bin/sh
read request
printf '%s\n' '{"schema_version":1,"capabilities":["identify_action"],"result":{"status":"handled","propagation":"stops","summary":"editor handled the key","details":["mode: normal"]}}'
"##,
    )
    .unwrap();
    let mut permissions = fs::metadata(&extension).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&extension, permissions).unwrap();

    let output = binary()
        .args(["extension", extension.to_str().unwrap(), "ctrl+x", "--json"])
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key_display"], "CTRL + X");
    assert_eq!(value["extension"]["capabilities"][0], "identify_action");
    assert_eq!(value["extension"]["layer"]["status"], "Handled");
    assert_eq!(value["extension"]["layer"]["propagation"], "Stops");
    let _ = fs::remove_dir_all(base);
}

#[test]
fn doctor_returns_structured_json() {
    let output = binary().args(["--json", "doctor"]).output().unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert!(value["tty"]["available"].is_boolean());
    assert!(value["hyprland"]["ipc"].is_boolean());
    assert!(value["hyprland"]["applicable"].is_boolean());
    assert!(value["sway"]["ipc"].is_boolean());
    assert!(value["i3"]["ipc"].is_boolean());
    assert!(value["gnome"]["ipc"].is_boolean());
    assert!(value["kde"]["ipc"].is_boolean());
    assert!(value["xfce"]["ipc"].is_boolean());
    assert!(value["cinnamon"]["ipc"].is_boolean());
    assert!(value["mate"]["ipc"].is_boolean());
    assert!(value["niri"]["ipc"].is_boolean());
    assert!(value["river"]["ipc"].is_boolean());
    assert!(value["wayfire"]["ipc"].is_boolean());
    assert!(value["labwc"]["ipc"].is_boolean());
    assert!(value["bspwm_sxhkd"]["ipc"].is_boolean());
    assert!(value["openbox"]["ipc"].is_boolean());
    assert!(value["compositor"]["applicable"].is_boolean());
    assert!(value["ghostty"]["effective_keybinds"].is_boolean());
    assert!(value["evdev"]["available"].is_boolean());
    assert!(value["evdev"]["devices"].is_array());
    assert!(value["remappers"].is_array());
    assert!(value["ime"].is_array());
    assert!(value["shell_snapshot"].is_boolean());
    assert!(value.get("terminal_adapter").is_some());
    assert!(value["extensions"]["whykey_nvim"]["installed"].is_boolean());
}

#[test]
fn doctor_schema_v2_includes_safe_next_steps() {
    let output = binary()
        .args(["--json", "--schema-version", "2", "doctor"])
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["operation"], "doctor");
    assert!(value["next_steps"].is_array());
    let next_steps = value["next_steps"].as_array().unwrap();
    assert!(
        next_steps
            .iter()
            .all(|step| step.as_str().is_some_and(|text| !text.contains("reload")))
    );
}

#[test]
fn doctor_reports_a_keyd_configuration_as_remapper_evidence() {
    let base = temp_dir("doctor-keyd");
    fs::create_dir_all(&base).unwrap();
    let config = base.join("keyd.conf");
    fs::write(&config, "[main]\ncapslock = overload(control, esc)\n").unwrap();

    let output = binary()
        .args(["--json", "doctor"])
        .env("WHYKEY_KEYD_CONFIG", &config)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let keyd = value["remappers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "keyd")
        .unwrap();
    assert!(
        keyd["transformations"][0]
            .as_str()
            .is_some_and(|value| value.contains("capslock -> overload"))
    );
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn ime_runtime_query_reports_the_active_fcitx5_engine_without_changing_it() {
    let base = temp_dir("doctor-ime-runtime");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marker = base.join("fcitx5-remote-ran");
    let remote = bin.join("fcitx5-remote");
    fs::write(
        &remote,
        format!(
            "#!/bin/sh\nprintf '%s' ran > '{}'\nexit 134\n",
            marker.display()
        ),
    )
    .unwrap();
    let dbus_send = bin.join("dbus-send");
    fs::write(
        &dbus_send,
        "#!/bin/sh\ncase \"$5\" in\n  org.freedesktop.DBus.ListNames) printf '%s\\n' 'org.fcitx.Fcitx5' ;;\n  org.fcitx.Fcitx.Controller1.CurrentInputMethod) printf '%s\\n' '   keyboard-us' ;;\n  org.fcitx.Fcitx.Controller1.State) printf '%s\\n' 'int32 2' ;;\n  *) exit 1 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&remote).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&remote, permissions.clone()).unwrap();
    fs::set_permissions(&dbus_send, permissions).unwrap();

    let output = binary()
        .args(["--json", "doctor"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", &bin)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fcitx = value["ime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["engine"] == "fcitx5")
        .unwrap();
    assert_eq!(fcitx["active_engine"], "keyboard-us");
    assert_eq!(fcitx["state"], "active");
    assert!(fcitx.get("query_error").is_none());
    assert!(!marker.exists(), "fcitx5-remote must not be launched");

    let inspection = binary()
        .args(["--json", "ctrl+a"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", &bin)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();
    let inspection: serde_json::Value = serde_json::from_slice(&inspection.stdout).unwrap();
    let ime_layer = inspection["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|layer| layer["layer"] == "Input method")
        .unwrap();
    assert_eq!(ime_layer["status"], "Indeterminate");
    assert!(
        ime_layer["details"]
            .as_array()
            .unwrap()
            .iter()
            .any(|detail| detail == "active engine: keyboard-us")
    );
    assert!(!marker.exists(), "fcitx5-remote must not be launched");

    let _ = fs::remove_dir_all(base);
}

#[test]
fn inspection_includes_detected_remapper_as_conditional_evidence() {
    let base = temp_dir("inspect-keyd");
    fs::create_dir_all(&base).unwrap();
    let config = base.join("keyd.conf");
    fs::write(&config, "[main]\ncapslock = overload(control, esc)\n").unwrap();

    let output = binary()
        .args(["ctrl+c"])
        .env("WHYKEY_KEYD_CONFIG", &config)
        .env("XDG_CURRENT_DESKTOP", "XFCE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Input remapper"));
    assert!(text.contains("capslock -> overload(control, esc)"));
    assert!(text.contains("remapper may transform"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn capabilities_expose_implemented_and_planned_entries() {
    let output = binary().args(["--json", "capabilities"]).output().unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    let capabilities = value["capabilities"].as_array().unwrap();
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "cli.inspect.sequence"
            && capability["implemented"] == true
            && capability["availability"] == "available"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "cli.replay"
            && capability["implemented"] == true
            && capability["availability"] == "available"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "cli.bindings"
            && capability["implemented"] == true
            && capability["availability"] == "available"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "cli.conflicts"
            && capability["implemented"] == true
            && capability["availability"] == "available"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "cli.extensions"
            && capability["implemented"] == true
            && capability["availability"] == "available"
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "input.ime-context" && capability["implemented"] == true
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "compositor.x11.xbindkeys" && capability["implemented"] == true
    }));
    assert!(capabilities.iter().any(|capability| {
        capability["id"] == "compositor.programmable-x11" && capability["implemented"] == true
    }));
}

#[test]
fn capabilities_text_is_human_readable() {
    let output = binary().arg("capabilities").output().unwrap();

    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("whykey capabilities"));
    assert!(text.contains("whykey extension"));
    assert!(text.contains("cli.inspect.sequence"));
}

#[test]
fn replay_renders_a_saved_json_report_without_injecting_input() {
    let base = temp_dir("replay");
    fs::create_dir_all(&base).unwrap();
    let source = binary().args(["--json", "ctrl+z"]).output().unwrap();
    let path = base.join("capture.json");
    fs::write(&path, source.stdout).unwrap();

    let output = binary()
        .args(["replay", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("whykey replay"));
    assert!(text.contains("no input was injected"));
    assert!(text.contains("CTRL + Z"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn replay_preserves_json_documents() {
    let base = temp_dir("replay-json");
    fs::create_dir_all(&base).unwrap();
    let source = binary().args(["--json", "ctrl+z"]).output().unwrap();
    let path = base.join("capture.json");
    fs::write(&path, source.stdout).unwrap();

    let output = binary()
        .args(["--json", "replay", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["key_display"], "CTRL + Z");
    let _ = fs::remove_dir_all(base);
}

#[test]
fn snapshot_writes_a_replayable_static_envelope() {
    let base = temp_dir("snapshot");
    fs::create_dir_all(&base).unwrap();
    let path = base.join("diagnostic.json");

    let output = binary()
        .args(["snapshot", "ctrl+z", "--output", path.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(matches!(output.status.code(), Some(0 | 1)));
    assert!(output.stdout.is_empty());
    let snapshot: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(snapshot["kind"], "whykey.diagnostic_snapshot");
    assert_eq!(snapshot["request"]["key_display"], "CTRL + Z");
    assert_eq!(snapshot["report"]["operation"], "inspect");

    let replay = binary()
        .args(["replay", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(replay.status.success());
    assert!(String::from_utf8_lossy(&replay.stdout).contains("CTRL + Z"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn diff_compares_saved_reports_without_querying_the_desktop() {
    let base = temp_dir("diff");
    fs::create_dir_all(&base).unwrap();
    let before = base.join("before.json");
    let after = base.join("after.json");
    let report = |summary: &str| {
        serde_json::json!({
            "schema_version": 1,
            "key": {"modifiers": 4, "key": "X"},
            "key_display": "CTRL + X",
            "confidence": "configured",
            "layers": [{
                "layer": "Hyprland",
                "status": "Handled",
                "propagation": "Stops",
                "summary": summary,
                "details": []
            }]
        })
    };
    fs::write(&before, serde_json::to_string(&report("before")).unwrap()).unwrap();
    fs::write(&after, serde_json::to_string(&report("after")).unwrap()).unwrap();

    let output = binary()
        .args(["diff", before.to_str().unwrap(), after.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("path[0].Hyprland"));
    assert!(text.contains("before"));
    assert!(text.contains("after"));

    let json_output = binary()
        .args([
            "--json",
            "diff",
            before.to_str().unwrap(),
            after.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(json_output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert_eq!(value["operation"], "diff");
    assert!(
        value["changes"]
            .as_array()
            .is_some_and(|changes| !changes.is_empty())
    );

    let v2_output = binary()
        .args([
            "--json",
            "--schema-version",
            "2",
            "diff",
            before.to_str().unwrap(),
            after.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(v2_output.status.success());
    let v2: serde_json::Value = serde_json::from_slice(&v2_output.stdout).unwrap();
    assert_eq!(v2["schema_version"], 2);
    let _ = fs::remove_dir_all(base);
}

#[test]
fn replay_can_upgrade_a_saved_v1_document_to_schema_v2() {
    let base = temp_dir("replay-upgrade-v2");
    fs::create_dir_all(&base).unwrap();
    let source = binary().args(["--json", "ctrl+z"]).output().unwrap();
    let path = base.join("capture.json");
    fs::write(&path, source.stdout).unwrap();

    let output = binary()
        .args([
            "--json",
            "--schema-version",
            "2",
            "replay",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["operation"], "inspect");
    assert!(value["path"].is_array());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn replay_renders_a_schema_v2_document() {
    let base = temp_dir("replay-v2");
    fs::create_dir_all(&base).unwrap();
    let source = binary()
        .args(["--json", "--schema-version", "2", "ctrl+z"])
        .output()
        .unwrap();
    let path = base.join("capture.json");
    fs::write(&path, source.stdout).unwrap();

    let output = binary()
        .args(["replay", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("CTRL + Z"));
    assert!(text.contains("whykey replay"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn replay_rejects_an_invalid_file() {
    let base = temp_dir("replay-invalid");
    fs::create_dir_all(&base).unwrap();
    let path = base.join("invalid.json");
    fs::write(&path, r#"{"schema_version":3}"#).unwrap();

    let output = binary()
        .args(["replay", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("schema_version 1 or 2"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn doctor_detects_gnu_screen_context() {
    let output = binary()
        .args(["--json", "doctor"])
        .env("STY", "1234.pts-0.host")
        .env_remove("TMUX")
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["multiplexer"], "screen");
}

#[test]
fn doctor_reports_nested_multiplexers() {
    let output = binary()
        .args(["--json", "doctor"])
        .env("TMUX", "/tmp/tmux-1000/default,1,0")
        .env("STY", "1234.pts-0.host")
        .env_remove("ZELLIJ")
        .env_remove("ZELLIJ_SESSION_NAME")
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["multiplexer"], "tmux + screen");
}

#[test]
fn remote_ssh_sessions_skip_local_hyprland_inspection() {
    let base = temp_dir("remote-ssh");
    fs::create_dir_all(&base).unwrap();
    let output = binary()
        .args(["--verbose", "ctrl+z"])
        .env("SSH_TTY", "/dev/pts/99")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SSH_CONNECTION")
        .env("PATH", &base)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("not applicable in this remote session"));
    assert!(!text.contains("failed to run hyprctl"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn ime_runtime_query_skips_fcitx_remote_when_session_bus_is_unavailable() {
    let base = temp_dir("doctor-ime-no-bus");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marker = base.join("fcitx5-remote-ran");
    let remote = bin.join("fcitx5-remote");
    fs::write(
        &remote,
        format!(
            "#!/bin/sh\nprintf '%s' ran > '{}'\nexit 134\n",
            marker.display()
        ),
    )
    .unwrap();
    let dbus_send = bin.join("dbus-send");
    fs::write(&dbus_send, "#!/bin/sh\nexit 1\n").unwrap();
    for command in [&remote, &dbus_send] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let output = binary()
        .args(["--json", "doctor"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", &bin)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    assert!(!marker.exists(), "fcitx5-remote must not be launched");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fcitx = value["ime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["engine"] == "fcitx5")
        .unwrap();
    assert!(
        fcitx["query_error"]
            .as_str()
            .is_some_and(|error| error.contains("skipped Fcitx5 runtime query"))
    );

    let inspection = binary()
        .args(["--json", "ctrl+a"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", &bin)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();
    assert_ne!(inspection.status.code(), Some(2));
    assert!(!marker.exists(), "inspect must not launch fcitx5-remote");
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn ime_runtime_query_reports_malformed_reply_without_launching_fcitx_remote() {
    let base = temp_dir("doctor-ime-dbus-runtime");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marker = base.join("fcitx5-remote-ran");
    let remote = bin.join("fcitx5-remote");
    fs::write(
        &remote,
        format!(
            "#!/bin/sh\nprintf '%s' ran > '{}'\nexit 134\n",
            marker.display()
        ),
    )
    .unwrap();
    let dbus_send = bin.join("dbus-send");
    fs::write(
        &dbus_send,
        "#!/bin/sh\ncase \"$5\" in\n  org.freedesktop.DBus.ListNames) printf '%s\\n' 'org.fcitx.Fcitx5' ;;\n  org.fcitx.Fcitx.Controller1.CurrentInputMethod) printf '%s\\n' 'int32 2' ;;\n  org.fcitx.Fcitx.Controller1.State) printf '%s\\n' 'int32 2' ;;\n  *) exit 1 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    for command in [&remote, &dbus_send] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let output = binary()
        .args(["--json", "doctor"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", &bin)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    assert!(!marker.exists(), "fcitx5-remote must not be launched");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fcitx = value["ime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["engine"] == "fcitx5")
        .unwrap();
    assert!(fcitx.get("active_engine").is_none());
    assert!(fcitx.get("state").is_none());
    assert!(
        fcitx["query_error"]
            .as_str()
            .is_some_and(|error| error.contains("no D-Bus string value"))
    );
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn ime_runtime_query_times_out_without_launching_fcitx_remote() {
    let base = temp_dir("doctor-ime-timeout");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let marker = base.join("fcitx5-remote-ran");
    let remote = bin.join("fcitx5-remote");
    fs::write(
        &remote,
        format!(
            "#!/bin/sh\nprintf '%s' ran > '{}'\nexit 134\n",
            marker.display()
        ),
    )
    .unwrap();
    let dbus_send = bin.join("dbus-send");
    fs::write(&dbus_send, "#!/bin/sh\nexec sleep 2\n").unwrap();
    for command in [&remote, &dbus_send] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let mut path = vec![bin.clone()];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
    let path = env::join_paths(path).unwrap();

    let output = binary()
        .args(["--json", "doctor"])
        .env("GTK_IM_MODULE", "fcitx")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/tmp/whykey-test-bus")
        .env("PATH", path)
        .env("HOME", &base)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("WHYKEY_COMMAND_TIMEOUT_MS", "50")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    assert_ne!(output.status.code(), Some(2));
    assert!(!marker.exists(), "fcitx5-remote must not be launched");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fcitx = value["ime"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["engine"] == "fcitx5")
        .unwrap();
    assert!(
        fcitx["query_error"]
            .as_str()
            .is_some_and(|error| error.contains("timed out")),
        "unexpected Fcitx5 timeout evidence: {}",
        fcitx["query_error"]
    );
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn stale_hyprland_ipc_keeps_the_report_conditional_and_continues() {
    let base = temp_dir("hyprland-stale-ipc");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();

    let hyprctl = bin.join("hyprctl");
    let stale_error = include_str!("fixtures/hyprland/hyprctl-stale.stderr").trim();
    fs::write(
        &hyprctl,
        format!("#!/bin/sh\nprintf '%s\\n' '{stale_error}' >&2\nexit 1\n"),
    )
    .unwrap();
    let ghostty = bin.join("ghostty");
    fs::write(&ghostty, "#!/bin/sh\nexit 0\n").unwrap();
    for command in [&hyprctl, &ghostty] {
        let mut permissions = fs::metadata(command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(command, permissions).unwrap();
    }

    let output = binary()
        .args(["--verbose", "ctrl+left"])
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("HOME", &base)
        .env("TERM_PROGRAM", "ghostty")
        .env("XDG_CURRENT_DESKTOP", "Hyprland")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("HYPRLAND_INSTANCE_SIGNATURE", "stale-instance")
        // The shell layer is ambient-sensitive (SHELL/inputrc); pin a Bash
        // snapshot so this Hyprland-conditional test stays deterministic.
        .env("SHELL", "/bin/bash")
        .env(
            "WHYKEY_READLINE_BINDINGS",
            "backward-word can be found on \"\\e[1;5D\".\n",
        )
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(text.contains("Hyprland IPC is unavailable"));
    assert!(text.contains("Ghostty"));
    assert!(text.contains("Bash / Readline"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn doctor_marks_hyprland_not_applicable_for_remote_ssh() {
    let output = binary()
        .args(["--json", "doctor"])
        .env("SSH_TTY", "/dev/pts/99")
        .env_remove("SSH_CONNECTION")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .output()
        .unwrap();

    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ssh"], true);
    assert_eq!(value["hyprland"]["ipc"], false);
    assert_eq!(value["hyprland"]["applicable"], false);
}

#[test]
fn evdev_requires_a_device_path_when_device_option_is_used() {
    let output = binary().args(["listen", "--device"]).output().unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--device requires a path"));
}

#[cfg(unix)]
#[test]
fn listen_without_a_controlling_tty_is_an_operational_failure() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let output = Command::new("setsid")
        .args(["--wait", executable, "listen", "--json", "--timeout", "0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("util-linux setsid must be available for the no-TTY test");
    assert_eq!(output.status.code(), Some(1), "output: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no controlling terminal"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn listen_rejects_invalid_capture_limits() {
    for (arguments, message) in [
        (
            &["listen", "--timeout", "0"][..],
            "--timeout must be a positive",
        ),
        (
            &["listen", "--count", "0"][..],
            "--count must be a positive",
        ),
        (
            &["listen", "--events", "press"][..],
            "unsupported event mode",
        ),
        (
            &["listen", "--terminal", "--events", "all"][..],
            "--events all is not supported with --terminal",
        ),
        (
            &["listen", "--terminal", "--evdev"][..],
            "--terminal and --evdev cannot be used together",
        ),
        (
            &["listen", "--timeout", "1e308"][..],
            "outside the supported duration range",
        ),
        (&["listen", "--output"][..], "--output requires a path"),
        (
            &["listen", "--output", "capture.json"][..],
            "--output requires --json or --ndjson",
        ),
    ] {
        let output = binary().args(arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "args: {arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "args: {arguments:?} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn listen_help_documents_capture_limits_and_detailed_events() {
    let output = binary().args(["listen", "--help"]).output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("--timeout"));
    assert!(text.contains("--count"));
    assert!(text.contains("--events all"));
    assert!(text.contains("--terminal"));
    assert!(text.contains("--ndjson"));
    assert!(text.contains("--output PATH"));
}

#[test]
fn ndjson_is_reserved_for_capture_streams() {
    let output = binary().args(["--ndjson", "ctrl+z"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("only supported by `whykey listen`"));
}

#[test]
fn no_arguments_prints_help_instead_of_entering_capture_mode() {
    let output = binary().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("whykey inspect"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Waiting for input"));
}

#[cfg(unix)]
fn script_command(command_line: &str) -> Command {
    let mut command = Command::new("script");
    command.args(["-qfec", command_line, "/dev/null"]);
    command.env_remove("HYPRLAND_INSTANCE_SIGNATURE");
    command
}

#[cfg(unix)]
struct PtySession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    prefix_output: Vec<u8>,
}

#[cfg(unix)]
impl PtySession {
    fn spawn(command_line: &str) -> Self {
        let mut child = script_command(command_line)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("util-linux script must provide a PTY for the listener test");

        let stdin = child.stdin.take().expect("script stdin");
        let mut session = Self {
            child,
            stdin,
            prefix_output: Vec::new(),
        };
        session.wait_for_ready();
        session
    }

    fn wait_for_ready(&mut self) {
        use std::os::unix::io::AsRawFd as _;

        let stdout_fd = self.child.stdout.as_ref().unwrap().as_raw_fd();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut chunk = [0u8; 512];
        let initial_len = self.prefix_output.len();

        let mut ready = false;
        while std::time::Instant::now() < deadline {
            let mut pfd = libc::pollfd {
                fd: stdout_fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let res = unsafe { libc::poll(&mut pfd, 1, 50) };
            if res > 0 && pfd.revents & libc::POLLIN != 0 {
                let n = unsafe { libc::read(stdout_fd, chunk.as_mut_ptr().cast(), chunk.len()) };
                if n > 0 {
                    self.prefix_output.extend_from_slice(&chunk[..n as usize]);
                    let slice = &self.prefix_output[initial_len..];
                    if slice.windows(18).any(|w| w == b"Waiting for input.") {
                        ready = true;
                        break;
                    }
                } else if n <= 0 {
                    break;
                }
            }
        }
        assert!(
            ready,
            "PTY listener failed to reach 'Waiting for input.' within deadline. Output received: {}",
            String::from_utf8_lossy(&self.prefix_output[initial_len..])
        );
    }

    fn write_input(&mut self, bytes: &[u8]) {
        self.stdin.write_all(bytes).unwrap();
        self.stdin.flush().unwrap();
    }

    fn wait(self) -> (std::process::Output, String) {
        let output = self.child.wait_with_output().unwrap();
        let mut combined_stdout = self.prefix_output;
        combined_stdout.extend_from_slice(&output.stdout);
        let transcript = format!(
            "{}{}",
            String::from_utf8_lossy(&combined_stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output, transcript)
    }
}

#[cfg(unix)]
#[test]
fn listen_exits_through_a_pty_after_kitty_escape() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --json --schema-version 2 --timeout 2",
        executable.replace('\'', "'\\''")
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[27;1u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");
    assert!(transcript.contains("whykey listen"));
    assert!(
        transcript.contains("Stopped."),
        "transcript: {transcript:?}"
    );
    assert!(
        transcript.contains("\x1b[<u"),
        "listener did not restore the Kitty protocol: {transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn listen_exits_through_a_pty_after_kitty_ctrl_c() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --json --timeout 2",
        executable.replace('\'', "'\\''")
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[99;5u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");
    let transcript_out = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = transcript_out;
    assert!(
        transcript.contains("Stopped."),
        "transcript: {transcript:?}"
    );
    assert!(transcript.contains("\x1b[<u"));
}

#[cfg(unix)]
#[test]
fn listen_captures_kitty_ctrl_z_through_a_pty() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --json --timeout 2",
        executable.replace('\'', "'\\''")
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[57442;5u\x1b[122;5u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");
    assert!(
        transcript.contains("\"key_display\": \"CTRL + Z\""),
        "transcript: {transcript:?}"
    );
    assert!(transcript.contains("Kitty keyboard protocol"));
    assert!(transcript.contains("\x1b[<u"));
}

#[cfg(unix)]
#[test]
fn listen_ignores_a_kitty_release_before_the_next_press() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --json --schema-version 2 --timeout 2",
        executable.replace('\'', "'\\''")
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[13;1:3u\x1b[122;5u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");
    assert!(transcript.contains("\"key_display\": \"CTRL + Z\""));
    assert!(!transcript.contains("\"key_display\": \"RETURN\""));
    assert!(transcript.contains("\"operation\": \"listen\""));
}

#[cfg(unix)]
#[test]
fn listen_ndjson_writes_one_compact_v2_record_to_stdout() {
    let base = temp_dir("listen-ndjson");
    fs::create_dir_all(&base).unwrap();
    let stream = base.join("events.ndjson");
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --ndjson --timeout 2 > '{}'",
        executable.replace('\'', "'\\''"),
        stream.display()
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[122;5u");
    let (output, _) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");

    let records = fs::read_to_string(&stream).unwrap();
    let lines: Vec<_> = records.lines().collect();
    assert_eq!(lines.len(), 1, "records: {records:?}");
    let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(record["schema_version"], 2);
    assert_eq!(record["operation"], "listen");
    assert_eq!(record["input"]["key_display"], "CTRL + Z");
    assert!(record["observation"].is_object());
    assert!(record["path"].is_array());

    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn listen_output_exports_a_replayable_capture_without_stdout_records() {
    let base = temp_dir("listen-output");
    fs::create_dir_all(&base).unwrap();
    let export = base.join("capture.ndjson");
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --ndjson --output '{}' --timeout 2",
        executable.replace('\'', "'\\''"),
        export.display()
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[122;5u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");

    let records = fs::read_to_string(&export).unwrap();
    let line = records.lines().next().expect("one exported record");
    let record: serde_json::Value = serde_json::from_str(line).unwrap();
    assert_eq!(record["schema_version"], 2);
    assert_eq!(record["operation"], "listen");
    assert_eq!(record["input"]["key_display"], "CTRL + Z");
    assert!(!transcript.contains("\"key_display\""));

    let replay = binary()
        .args(["replay", export.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(replay.status.success(), "replay output: {replay:?}");
    assert!(String::from_utf8_lossy(&replay.stdout).contains("CTRL + Z"));

    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn listen_repeat_restores_between_reports() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --json --repeat --count 2 --timeout 3",
        executable.replace('\'', "'\\''")
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[122;5u");
    pty.wait_for_ready();
    pty.write_input(b"\x1b[122;5u");
    let (output, transcript) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");
    assert_eq!(
        transcript.matches("\"key_display\": \"CTRL + Z\"").count(),
        2,
        "transcript: {transcript:?}"
    );
    assert_eq!(
        transcript.matches("Waiting for input...").count(),
        2,
        "each report must start a fresh, restored capture cycle: {transcript:?}"
    );
    assert_eq!(transcript.matches("\x1b[<u").count(), 2);
}

#[cfg(unix)]
fn high_rate_capture_records(event_count: usize) -> Result<String, String> {
    let base = temp_dir("listen-high-rate");
    fs::create_dir_all(&base).unwrap();
    let stream = base.join("events.ndjson");
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --ndjson --repeat --count {event_count} --timeout 90 > '{}'",
        executable.replace('\'', "'\\''"),
        stream.display()
    );
    let mut child = script_command(&command_line)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("util-linux script must provide a PTY for the high-rate listener test");
    // Give `script` and whykey time to create the PTY and enter raw capture
    // mode before injecting a burst. Without this, the shell can echo the
    // first bytes instead of delivering them to the listener.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut stdin = child.stdin.take().expect("script stdin");
    let writer = std::thread::spawn(move || {
        let event = b"\x1b[122;5u";
        let chunk_size = 100;
        let mut remaining = event_count;
        let mut error = None;
        while remaining > 0 {
            let count = remaining.min(chunk_size);
            let mut chunk = Vec::with_capacity(count * event.len());
            for _ in 0..count {
                chunk.extend_from_slice(event);
            }
            if let Err(write_error) = stdin.write_all(&chunk) {
                error = Some(write_error.to_string());
                break;
            }
            remaining -= count;
        }
        error
    });
    let output = child.wait_with_output().unwrap();
    let writer_error = writer.join().expect("high-rate writer thread");

    let records = fs::read_to_string(&stream).unwrap();
    let _ = fs::remove_dir_all(base);
    if output.status.success() {
        Ok(records)
    } else {
        Err(format!(
            "script output: {output:?}; writer: {writer_error:?}; records: {}",
            records.lines().count()
        ))
    }
}

#[cfg(unix)]
#[test]
#[ignore = "interactive capture restores after every report; bulk streaming is not a release capability"]
fn listen_sustains_a_bounded_high_rate_stream() {
    let event_count = 1_000;
    let records = high_rate_capture_records(event_count).unwrap();
    let lines: Vec<_> = records.lines().collect();
    assert_eq!(
        lines.len(),
        event_count,
        "captured records: {}",
        lines.len()
    );
    for line in lines {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(record["operation"], "listen");
        assert_eq!(record["input"]["key_display"], "CTRL + Z");
    }
}

#[cfg(unix)]
#[test]
#[ignore = "sustained capture gate; run explicitly when validating throughput"]
fn listen_sustains_60000_events() {
    let event_count = 60_000;
    let records = high_rate_capture_records(event_count).unwrap();
    assert_eq!(records.lines().count(), event_count);
}

#[cfg(unix)]
fn paced_capture_latencies(event_count: usize) -> Vec<std::time::Duration> {
    let base = temp_dir("listen-latency");
    fs::create_dir_all(&base).unwrap();
    let stream = base.join("events.ndjson");
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --ndjson --repeat --count {event_count} --timeout 10 > '{}'",
        executable.replace('\'', "'\\''"),
        stream.display()
    );
    let mut child = script_command(&command_line)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("util-linux script must provide a PTY for the latency gate");

    std::thread::sleep(std::time::Duration::from_millis(150));
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(event_count)));
    let reader_stream = stream.clone();
    let reader_sent = std::sync::Arc::clone(&sent);
    let reader = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut offset = 0;
        let mut latencies = Vec::with_capacity(event_count);
        while latencies.len() < event_count && std::time::Instant::now() < deadline {
            if let Ok(bytes) = fs::read(&reader_stream) {
                if bytes.len() > offset {
                    let new_bytes = &bytes[offset..];
                    if let Some(last_newline) = new_bytes.iter().rposition(|byte| *byte == b'\n') {
                        let complete_end = offset + last_newline + 1;
                        let records = new_bytes[..=last_newline]
                            .iter()
                            .filter(|byte| **byte == b'\n')
                            .count();
                        let now = std::time::Instant::now();
                        let sent = reader_sent.lock().unwrap();
                        let first = latencies.len();
                        let last = (first + records).min(sent.len());
                        latencies
                            .extend(sent[first..last].iter().map(|timestamp| now - *timestamp));
                        offset = complete_end;
                    }
                }
            }
            if latencies.len() < event_count {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        latencies
    });

    let mut stdin = child.stdin.take().expect("script stdin");
    let writer_sent = std::sync::Arc::clone(&sent);
    let writer = std::thread::spawn(move || {
        let event = b"\x1b[122;5u";
        let chunk_size = 10;
        let mut remaining = event_count;
        while remaining > 0 {
            let count = remaining.min(chunk_size);
            let mut chunk = Vec::with_capacity(count * event.len());
            for _ in 0..count {
                chunk.extend_from_slice(event);
            }
            let timestamp = std::time::Instant::now();
            writer_sent
                .lock()
                .unwrap()
                .extend(std::iter::repeat_n(timestamp, count));
            stdin
                .write_all(&chunk)
                .expect("latency writer must reach the listener");
            remaining -= count;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });

    let output = child.wait_with_output().unwrap();
    writer.join().expect("latency writer thread");
    let latencies = reader.join().expect("latency reader thread");
    assert!(output.status.success(), "script output: {output:?}");
    let _ = fs::remove_dir_all(base);
    latencies
}

#[cfg(unix)]
#[test]
#[ignore = "capture latency measurement; run explicitly when validating scheduling"]
fn listen_capture_p95_latency_is_measured() {
    let mut latencies = paced_capture_latencies(1_000);
    assert_eq!(latencies.len(), 1_000);
    latencies.sort_unstable();
    let p95_index = (latencies.len() * 95).div_ceil(100) - 1;
    let p95 = latencies[p95_index];
    let p50 = latencies[latencies.len() / 2];
    assert!(
        p95 < std::time::Duration::from_secs(1),
        "capture p95 latency measurement exceeded the sanity bound: {p95:?}"
    );
    println!("capture p50 latency: {p50:?}; p95 latency: {p95:?}");
}

#[cfg(unix)]
#[test]
fn listen_timeout_restores_the_pty_protocol() {
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --json --timeout 0.2",
        executable.replace('\'', "'\\''")
    );
    let mut child = script_command(&command_line)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("util-linux script must provide a PTY for the listener test");
    let _stdin = child.stdin.take().expect("script stdin");
    let output = child.wait_with_output().unwrap();
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        transcript.contains("capture timed out"),
        "transcript: {transcript:?}"
    );
    assert!(
        transcript.contains("\x1b[<u"),
        "listener did not restore the Kitty protocol: {transcript:?}"
    );
}

#[cfg(unix)]
fn listener_transcript_after_signal(signal: libc::c_int) -> String {
    use std::os::unix::process::CommandExt;

    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "exec '{}' listen --json --timeout 2",
        executable.replace('\'', "'\\''")
    );
    let mut command = script_command(&command_line);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Keep the signal scoped to script and the listener it launches; the
        // test runner must not receive the synthetic Ctrl+C.
        .process_group(0);
    let mut child = command
        .spawn()
        .expect("util-linux script must provide a PTY for the listener test");
    let _stdin = child.stdin.take().expect("script stdin");
    let script_pid = child.id();
    let listener_pid = (0..20).find_map(|_| {
        let pid = fs::read_to_string(format!("/proc/{script_pid}/task/{script_pid}/children"))
            .ok()
            .and_then(|children| {
                children
                    .split_whitespace()
                    .next()
                    .and_then(|child| child.parse().ok())
            });
        if pid.is_none() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        pid
    });
    let Some(listener_pid) = listener_pid else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("script must expose the listener child");
    };
    // Let the listener finish termios/protocol setup before delivering the
    // signal; otherwise the interrupt can legitimately occur during startup.
    std::thread::sleep(std::time::Duration::from_millis(150));
    // SAFETY: the PID comes from the script process's kernel-maintained child
    // list and identifies the listener, not the test runner.
    let result = unsafe { libc::kill(listener_pid, signal) };
    assert_eq!(result, 0, "failed to interrupt listener");
    let output = child.wait_with_output().unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[cfg(unix)]
#[test]
fn listen_sigint_restores_the_pty_protocol() {
    let transcript = listener_transcript_after_signal(libc::SIGINT);
    assert!(
        transcript.contains("terminal settings restored"),
        "transcript: {transcript:?}"
    );
    assert!(
        transcript.contains("\x1b[<u"),
        "listener did not restore the Kitty protocol: {transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn listen_sigterm_restores_the_pty_protocol() {
    let transcript = listener_transcript_after_signal(libc::SIGTERM);
    assert!(
        transcript.contains("terminal settings restored"),
        "transcript: {transcript:?}"
    );
    assert!(
        transcript.contains("\x1b[<u"),
        "listener did not restore the Kitty protocol: {transcript:?}"
    );
}

#[test]
fn completions_emit_a_shell_script() {
    for shell in ["bash", "zsh", "fish"] {
        let output = binary().args(["completions", shell]).output().unwrap();
        assert!(output.status.success(), "completion shell: {shell}");
        let text = String::from_utf8_lossy(&output.stdout);
        for command in [
            "inspect",
            "listen",
            "doctor",
            "capabilities",
            "bindings",
            "conflicts",
            "extension",
            "replay",
            "shell-init",
            "completions",
        ] {
            assert!(
                text.contains(command),
                "{shell} completion missing {command}"
            );
        }
        assert!(
            text.contains("ndjson"),
            "{shell} completion missing ndjson: {text}"
        );
        for placeholder in [
            "__COMMANDS__",
            "__SHELLS__",
            "__EXAMPLES__",
            "__COMMON_OPTIONS__",
            "__INSPECT_OPTIONS__",
            "__LISTEN_OPTIONS__",
        ] {
            assert!(
                !text.contains(placeholder),
                "{shell} completion has unresolved {placeholder}"
            );
        }
    }
}

#[test]
fn help_advertises_sequence_inspection() {
    let output = binary().arg("--help").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("whykey inspect"));
    assert!(text.contains("--pid PID"));
    assert!(text.contains("--focused"));
    assert!(text.contains("whykey capabilities"));
    assert!(text.contains("whykey replay"));
    assert!(text.contains("ctrl+x ctrl+s"));
}

#[test]
fn fish_shell_init_captures_the_active_mode() {
    let output = binary().args(["shell-init", "fish"]).output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("bind --all"));
    assert!(text.contains("WHYKEY_FISH_MODE $fish_bind_mode"));
}

#[test]
fn kitty_adapter_is_used_when_terminal_is_kitty() {
    let base = temp_dir("kitty-cli");
    let kitty = base.join("kitty");
    fs::create_dir_all(&kitty).unwrap();
    fs::write(
        kitty.join("kitty.conf"),
        "map ctrl+insert copy_to_clipboard\n",
    )
    .unwrap();

    let output = binary()
        .args(["ctrl+insert"])
        .env("TERM_PROGRAM", "kitty")
        .env("XDG_CONFIG_HOME", &base)
        .env("HOME", PathBuf::from(&base))
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Kitty consumes the key"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn alacritty_yaml_adapter_is_used_when_configured() {
    let base = temp_dir("alacritty-cli");
    fs::create_dir_all(&base).unwrap();
    let config = base.join("alacritty.yml");
    fs::write(
        &config,
        "key_bindings:\n  - { key: Left, mods: Control, chars: \"\\e[1;5D\" }\n",
    )
    .unwrap();

    let output = binary()
        .args(["ctrl+left"])
        .env("TERM_PROGRAM", "alacritty")
        .env("ALACRITTY_CONFIG_FILE", &config)
        .env("XDG_CONFIG_HOME", &base)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Alacritty sends a sequence to the PTY"));
    assert!(text.contains("sequence: ESC [ 1 ; 5 D"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn wezterm_adapter_uses_effective_key_command() {
    let base = temp_dir("wezterm-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let wezterm = bin.join("wezterm");
    fs::write(
        &wezterm,
        "#!/bin/sh\nprintf '%s\\n' 'Default key table' '-----------------' '    CTRL                 c                ->   CopyTo=\"Clipboard\"'",
    )
    .unwrap();
    let mut permissions = fs::metadata(&wezterm).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wezterm, permissions).unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), path.to_string_lossy());
    let output = binary()
        .args(["ctrl+c"])
        .env("TERM_PROGRAM", "WezTerm")
        .env("PATH", path)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("WezTerm consumes the key"));
    assert!(text.contains("CopyTo=\"Clipboard\""));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn sway_adapter_reads_effective_bindings() {
    let base = temp_dir("sway-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let swaymsg = bin.join("swaymsg");
    fs::write(
        &swaymsg,
        include_str!("fixtures/sessions/swaymsg-bindings.sh"),
    )
    .unwrap();
    let mut permissions = fs::metadata(&swaymsg).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&swaymsg, permissions).unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), path.to_string_lossy());
    let output = binary()
        .args(["ctrl+c"])
        .env("SWAYSOCK", "/tmp/whykey-test-sway.sock")
        .env("XDG_CURRENT_DESKTOP", "sway")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .env("PATH", path)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Sway"));
    assert!(text.contains("active binding found"));
    assert!(text.contains("exec copy"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn i3_adapter_reads_effective_bindings() {
    let base = temp_dir("i3-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let i3msg = bin.join("i3-msg");
    fs::write(&i3msg, include_str!("fixtures/sessions/i3-msg-bindings.sh")).unwrap();
    let mut permissions = fs::metadata(&i3msg).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&i3msg, permissions).unwrap();

    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), path.to_string_lossy());
    let output = binary()
        .args(["ctrl+c"])
        .env("I3SOCK", "/tmp/whykey-test-i3.sock")
        .env("XDG_CURRENT_DESKTOP", "i3")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env("PATH", path)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("i3"));
    assert!(text.contains("active binding found"));
    assert!(text.contains("exec copy"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn generic_desktop_is_reported_without_a_hyprland_error() {
    let base = temp_dir("desktop-context-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.to_string_lossy().into_owned();
    let output = binary()
        .args(["ctrl+c"])
        .env("XDG_CURRENT_DESKTOP", "LXQt")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env("PATH", path)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Desktop compositor"));
    assert!(text.contains("LXQt (wayland) detected"));
    assert!(!text.contains("failed to run hyprctl"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn virtual_session_matrix_covers_supported_desktop_adapters() {
    let value: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/sessions/matrix.json")).unwrap();
    assert_eq!(value["schema_version"], 1);
    let entries = value["entries"].as_array().unwrap();
    let expected = [
        "hyprland",
        "sway",
        "i3",
        "gnome",
        "kde",
        "xfce",
        "cinnamon",
        "mate",
        "niri",
        "river",
        "wayfire",
        "labwc",
        "bspwm-sxhkd",
        "openbox",
        "x11.xbindkeys",
        "awesome",
        "qtile",
        "xmonad",
        "generic",
    ];
    let source = include_str!("cli.rs");
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sessions");
    let mut seen = HashSet::new();

    for entry in entries {
        let adapter = entry["adapter"].as_str().unwrap();
        assert!(
            expected.contains(&adapter),
            "fixture matrix contains unknown adapter {adapter}"
        );
        assert!(
            seen.insert(adapter),
            "duplicate fixture entry for {adapter}"
        );
        let test_name = entry["test"].as_str().unwrap();
        assert!(
            source.contains(&format!("fn {test_name}(")),
            "fixture matrix test {test_name} is not present in tests/cli.rs"
        );
        for fixture in entry["fixtures"].as_array().unwrap() {
            let fixture = fixture.as_str().unwrap();
            let path = fixture_root.join(fixture);
            assert!(path.is_file(), "missing virtual-session fixture {fixture}");
        }
    }

    assert_eq!(seen.len(), expected.len());
    for adapter in expected {
        assert!(
            seen.contains(adapter),
            "missing fixture entry for {adapter}"
        );
    }
}

#[cfg(unix)]
#[test]
fn xfce_adapter_reads_effective_channel_shortcuts() {
    let base = temp_dir("xfce-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let xfconf = bin.join("xfconf-query");
    fs::write(
        &xfconf,
        include_str!("fixtures/sessions/xfconf-query-shortcuts.sh"),
    )
    .unwrap();
    let mut permissions = fs::metadata(&xfconf).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&xfconf, permissions).unwrap();

    let output = binary()
        .args(["super+r"])
        .env("XDG_CURRENT_DESKTOP", "XFCE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .env("PATH", bin.to_string_lossy().into_owned())
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Xfce"));
    assert!(text.contains("global shortcut consumes the key"));
    assert!(text.contains("xfrun4"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn niri_adapter_reads_literal_kdl_bindings() {
    let base = temp_dir("niri-cli");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let config_dir = base.join("niri");
    fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("config.kdl");
    fs::write(&config, include_str!("fixtures/sessions/niri-config.kdl")).unwrap();

    let output = binary()
        .args(["super+return"])
        .env("XDG_CURRENT_DESKTOP", "niri")
        .env("XDG_SESSION_DESKTOP", "niri")
        .env("XDG_CONFIG_HOME", &base)
        .env("PATH", &bin)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Niri"));
    assert!(text.contains("config contains a matching binding"));
    assert!(text.contains("spawn"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn river_adapter_reads_literal_init_map_commands() {
    let base = temp_dir("river-cli");
    let config_dir = base.join("river");
    fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("init");
    fs::write(&config, include_str!("fixtures/sessions/river-init")).unwrap();

    let output = binary()
        .args(["super+return"])
        .env("XDG_CURRENT_DESKTOP", "river")
        .env("XDG_SESSION_DESKTOP", "river")
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", &base)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("River"));
    assert!(text.contains("matching map command"));
    assert!(text.contains("spawn foot"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn wayfire_adapter_reads_literal_ini_bindings() {
    let base = temp_dir("wayfire-cli");
    fs::create_dir_all(&base).unwrap();
    let config = base.join("wayfire.ini");
    fs::write(&config, include_str!("fixtures/sessions/wayfire.ini")).unwrap();

    let output = binary()
        .args(["super+shift+t"])
        .env("XDG_CURRENT_DESKTOP", "wayfire")
        .env("XDG_SESSION_DESKTOP", "wayfire")
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", &base)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Wayfire"));
    assert!(text.contains("matching binding"));
    assert!(text.contains("terminal"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn labwc_adapter_reads_openbox_compatible_keybinds() {
    let base = temp_dir("labwc-cli");
    let config_dir = base.join("labwc");
    fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("rc.xml");
    fs::write(&config, include_str!("fixtures/sessions/labwc-rc.xml")).unwrap();

    let output = binary()
        .args(["super+ctrl+t"])
        .env("XDG_CURRENT_DESKTOP", "labwc")
        .env("XDG_SESSION_DESKTOP", "labwc")
        .env("PATH", "/nonexistent")
        .env("XDG_CONFIG_HOME", &base)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("labwc"));
    assert!(text.contains("matching labwc keybind"));
    assert!(text.contains("Execute: foot"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn xfce_bindings_inventory_is_versioned() {
    let base = temp_dir("bindings-xfce-cli");
    let bin = write_mock_bin(
        &base,
        "xfconf-query",
        include_str!("fixtures/sessions/xfconf-query-inventory.sh"),
    );

    let output = binary()
        .args(["--json", "bindings"])
        .env("XDG_CURRENT_DESKTOP", "XFCE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("PATH", bin_path(&bin))
        .env("HOME", &base)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["bindings"][0]["source"], "Xfce");
    assert_eq!(value["bindings"][0]["key"], "CTRL+ALT+T");
    assert_eq!(value["bindings"][0]["action"], "xfce4-terminal");
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn gnome_adapter_reads_media_key_bindings() {
    let base = temp_dir("gnome-cli");
    let bin = write_mock_bin(
        &base,
        "gsettings",
        include_str!("fixtures/sessions/gsettings-gnome.sh"),
    );

    let output = binary()
        .args(["super+l"])
        .env("XDG_CURRENT_DESKTOP", "GNOME")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("PATH", bin_path(&bin))
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("GNOME"));
    assert!(text.contains("global shortcut consumes the key"));
    assert!(text.contains("screensaver"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn cinnamon_adapter_reads_gsettings_bindings() {
    let base = temp_dir("cinnamon-cli");
    let bin = write_mock_bin(
        &base,
        "gsettings",
        include_str!("fixtures/sessions/gsettings-cinnamon.sh"),
    );

    let output = binary()
        .args(["super+1"])
        .env("XDG_CURRENT_DESKTOP", "Cinnamon")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("PATH", bin_path(&bin))
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Cinnamon"));
    assert!(text.contains("global shortcut consumes the key"));
    assert!(text.contains("switch-to-workspace-1"));
    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn mate_adapter_reads_marco_global_keybindings() {
    let base = temp_dir("mate-cli");
    let bin = write_mock_bin(
        &base,
        "gsettings",
        include_str!("fixtures/sessions/gsettings-mate.sh"),
    );

    let output = binary()
        .args(["alt+f2"])
        .env("XDG_CURRENT_DESKTOP", "MATE")
        .env("XDG_SESSION_TYPE", "x11")
        .env("PATH", bin_path(&bin))
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("MATE"));
    assert!(text.contains("global shortcut consumes the key"));
    assert!(text.contains("run-command-1"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn sxhkd_adapter_reads_static_binding_with_conditional_runtime() {
    let base = temp_dir("sxhkd-cli");
    let config_dir = base.join("sxhkd");
    fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("sxhkdrc");
    fs::write(&config, include_str!("fixtures/sessions/sxhkdrc")).unwrap();

    let output = binary()
        .args(["super+return"])
        .env("XDG_CURRENT_DESKTOP", "bspwm")
        .env("XDG_SESSION_DESKTOP", "bspwm")
        .env("XDG_SESSION_TYPE", "x11")
        .env("XDG_CONFIG_HOME", &base)
        .env("PATH", "/nonexistent")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("sxhkd"));
    assert!(text.contains("sxhkd has a matching binding"));
    assert!(text.contains("alacritty"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn openbox_adapter_reads_xml_keybinds_conditionally() {
    let base = temp_dir("openbox-cli");
    let config_dir = base.join("openbox");
    fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("rc.xml");
    fs::write(&config, include_str!("fixtures/sessions/openbox-rc.xml")).unwrap();

    let output = binary()
        .args(["super+ctrl+t"])
        .env("XDG_CURRENT_DESKTOP", "Openbox")
        .env("XDG_SESSION_DESKTOP", "Openbox")
        .env("XDG_SESSION_TYPE", "x11")
        .env("XDG_CONFIG_HOME", &base)
        .env("PATH", "/nonexistent")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Openbox"));
    assert!(text.contains("Openbox has a matching binding"));
    assert!(text.contains("Execute: alacritty"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn kde_adapter_reads_global_shortcut_configuration() {
    let base = temp_dir("kde-cli");
    fs::create_dir_all(&base).unwrap();
    let config = base.join("kglobalshortcutsrc");
    fs::write(
        &config,
        include_str!("fixtures/sessions/kglobalshortcutsrc"),
    )
    .unwrap();

    let output = binary()
        .args(["alt+f2"])
        .env("XDG_CURRENT_DESKTOP", "KDE")
        .env("XDG_SESSION_DESKTOP", "KDE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CONFIG_HOME", &base)
        .env("PATH", "/nonexistent")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("KDE Plasma"));
    assert!(text.contains("matching KDE global shortcut configured"));
    assert!(text.contains("Run Command"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn x11_adapter_reads_literal_xbindkeys_configuration() {
    let base = temp_dir("x11-xbindkeys-cli");
    fs::create_dir_all(&base).unwrap();
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let config = base.join(".xbindkeysrc");
    fs::write(&config, include_str!("fixtures/sessions/xbindkeysrc")).unwrap();

    let output = binary()
        .args(["ctrl+alt+t"])
        .env("PATH", &bin)
        .env("XBINDKEYSRC", &config)
        .env("XDG_CURRENT_DESKTOP", "generic")
        .env("XDG_SESSION_DESKTOP", "generic")
        .env("XDG_SESSION_TYPE", "x11")
        .env("DISPLAY", ":0")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("X11 xbindkeys"));
    assert!(text.contains("matching xbindkeys shortcut configured"));
    assert!(text.contains("alacritty"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn programmable_x11_adapters_read_literal_bindings() {
    let cases = [
        (
            "awesome",
            "AWESOME_CONFIG",
            "rc.lua",
            include_str!("fixtures/sessions/awesome-rc.lua"),
            "super+c",
            "AwesomeWM",
        ),
        (
            "qtile",
            "QTILE_CONFIG",
            "config.py",
            include_str!("fixtures/sessions/qtile-config.py"),
            "super+shift+h",
            "Qtile",
        ),
        (
            "xmonad",
            "XMONAD_CONFIG",
            "xmonad.hs",
            include_str!("fixtures/sessions/xmonad.hs"),
            "super+shift+c",
            "XMonad",
        ),
    ];
    for (desktop, config_var, file_name, content, key, label) in cases {
        let base = temp_dir(&format!("programmable-{desktop}"));
        fs::create_dir_all(base.join("bin")).unwrap();
        let config = base.join(file_name);
        fs::write(&config, content).unwrap();
        let output = binary()
            .args([key])
            .env("PATH", base.join("bin"))
            .env("XDG_CURRENT_DESKTOP", desktop)
            .env("XDG_SESSION_DESKTOP", desktop)
            .env("XDG_SESSION_TYPE", "x11")
            .env("DISPLAY", ":0")
            .env(config_var, &config)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("SWAYSOCK")
            .env_remove("I3SOCK")
            .env_remove("SSH_CONNECTION")
            .env_remove("SSH_TTY")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains(label), "{desktop}: {text}");
        assert!(text.contains("matching"), "{desktop}: {text}");
        let _ = fs::remove_dir_all(base);
    }
}

#[test]
fn bindings_command_emits_a_versioned_kde_inventory() {
    let base = temp_dir("bindings-kde-cli");
    fs::create_dir_all(&base).unwrap();
    fs::write(
        base.join("kglobalshortcutsrc"),
        "[org.kde.krunner.desktop]\n_launch=Alt+F2,none,Run Command\n",
    )
    .unwrap();

    let output = binary()
        .args(["--json", "bindings"])
        .env("XDG_CURRENT_DESKTOP", "KDE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CONFIG_HOME", &base)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("GNOME_DESKTOP_SESSION_ID")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["complete"], false);
    assert_eq!(value["bindings"][0]["source"], "KDE Plasma");
    assert_eq!(value["bindings"][0]["key"], "ALT+F2");
    assert!(
        value["bindings"][0]["action"]
            .as_str()
            .is_some_and(|action| action.contains("Run Command"))
    );
    let _ = fs::remove_dir_all(base);
}

#[test]
fn conflicts_command_keeps_same_context_collisions_explicit() {
    let base = temp_dir("conflicts-kde-cli");
    fs::create_dir_all(&base).unwrap();
    fs::write(
        base.join("kglobalshortcutsrc"),
        "[component.one]\naction_one=Alt+F2,none,First\naction_two=Alt+F2,none,Second\n",
    )
    .unwrap();

    let output = binary()
        .args(["--json", "conflicts"])
        .env("XDG_CURRENT_DESKTOP", "KDE")
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_CONFIG_HOME", &base)
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("SWAYSOCK")
        .env_remove("I3SOCK")
        .env_remove("GNOME_DESKTOP_SESSION_ID")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(value["conflicts"][0]["key"], "ALT+F2");
    assert_eq!(
        value["conflicts"][0]["classification"],
        "possible; runtime activation or precedence is unknown"
    );
    let _ = fs::remove_dir_all(base);
}

#[test]
fn konsole_adapter_reads_explicit_keytab() {
    let base = temp_dir("konsole-cli");
    fs::create_dir_all(&base).unwrap();
    let keytab = base.join("custom.keytab");
    fs::write(
        &keytab,
        "keyboard \"custom\"\nkey Left +Ctrl : \"\\E[1;5D\"\n",
    )
    .unwrap();

    let output = binary()
        .args(["ctrl+left"])
        .env("TERM_PROGRAM", "konsole")
        .env("KONSOLE_KEYTAB", &keytab)
        .env("HOME", &base)
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Konsole sends a sequence to the PTY"));
    assert!(text.contains("sequence: ESC [ 1 ; 5 D"));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn default_source_selection_falls_back_to_terminal_without_hyprland() {
    let output = binary()
        .args(["listen", "--timeout", "0.01"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("Global Hyprland capture is unavailable:"));
    assert!(
        combined.contains(
            "Using terminal capture; shortcuts consumed by the compositor will not appear."
        )
    );
}

#[test]
fn terminal_flag_suppresses_fallback_warning() {
    let output = binary()
        .args(["listen", "--terminal", "--timeout", "0.01"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined.contains("Global Hyprland capture is unavailable"));
}

#[test]
fn listen_events_all_fails_when_hyprland_unavailable() {
    let output = binary()
        .args(["listen", "--events", "all"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--events all requires Hyprland or evdev capture"));
    assert!(stderr.contains("HYPRLAND_INSTANCE_SIGNATURE is not set"));
}

#[test]
fn listen_json_fallback_warns_on_stderr_without_polluting_stdout() {
    let output = binary()
        .args(["listen", "--json", "--timeout", "0.01"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Global Hyprland capture is unavailable"));
    assert!(
        stderr.contains(
            "Using terminal capture; shortcuts consumed by the compositor will not appear."
        )
    );
}

#[test]
fn hyprland_suppression_failure_fails_closed() {
    let output = binary()
        .env("HYPRLAND_INSTANCE_SIGNATURE", "nonexistent_fake_sig_12345")
        .args(["listen", "--timeout", "0.01"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("could not suppress Hyprland shortcuts"));
    assert!(stderr.contains("No key was captured and no shortcut was executed."));
    assert!(stderr.contains("Use --pass-through to capture without suppression."));
}

#[test]
fn pass_through_flag_allows_fallback_when_hyprland_fails() {
    let output = binary()
        .env("HYPRLAND_INSTANCE_SIGNATURE", "nonexistent_fake_sig_12345")
        .args(["listen", "--pass-through", "--timeout", "0.01"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("Global Hyprland capture is unavailable"));
    assert!(combined.contains("Using terminal capture"));
}

#[cfg(unix)]
#[test]
fn listen_terminal_mode_claims_observed_only_disposition() {
    let base = temp_dir("listen-disposition");
    fs::create_dir_all(&base).unwrap();
    let stream = base.join("events.ndjson");
    let executable = env!("CARGO_BIN_EXE_whykey");
    let command_line = format!(
        "'{}' listen --terminal --ndjson --timeout 2 > '{}'",
        executable.replace('\'', "'\\''"),
        stream.display()
    );
    let mut pty = PtySession::spawn(&command_line);
    pty.write_input(b"\x1b[122;5u");
    let (output, _) = pty.wait();
    assert!(output.status.success(), "script output: {output:?}");

    let records = fs::read_to_string(&stream).unwrap();
    let lines: Vec<_> = records.lines().collect();
    assert_eq!(lines.len(), 1, "records: {records:?}");
    let record: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(record["observation"]["disposition"], "observed_only");

    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_with_unset_and_empty_shell_preserves_matches_and_exits_zero() {
    let base = temp_dir("hyprland-shell-test");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let hyprctl = bin.join("hyprctl");
    fs::write(
        &hyprctl,
        "#!/bin/sh\ncase \"$*\" in *instances*) printf '%s\\n' '[{\"instance\":\"test\",\"wl_socket\":\"wayland-1\"}]' ;; *binds*) cat tests/fixtures/hyprland/binds-representative.json ;; *submap*) printf '%s\\n' '\"default\"' ;; *devices*) printf '%s\\n' '{\"keyboards\":[]}' ;; *) exit 0 ;; esac\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&hyprctl).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&hyprctl, permissions).unwrap();

    // Test with unset SHELL
    let output_unset = binary()
        .args(["inspect", "super+c", "--instance", "test"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), env::var("PATH").unwrap_or_default()),
        )
        .env_remove("SHELL")
        .env_remove("WHYKEY_READLINE_BINDINGS")
        .output()
        .unwrap();

    assert_eq!(
        output_unset.status.code(),
        Some(0),
        "unset SHELL must not cause non-zero exit when upstream matches: {output_unset:?}"
    );
    let text_unset = String::from_utf8_lossy(&output_unset.stdout);
    assert!(text_unset.contains("Hyprland"));
    assert!(!text_unset.contains("Result:\n  Could not inspect Shell input."));

    // Test with empty SHELL=""
    let output_empty = binary()
        .args(["inspect", "super+c", "--instance", "test"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), env::var("PATH").unwrap_or_default()),
        )
        .env("SHELL", "")
        .env_remove("WHYKEY_READLINE_BINDINGS")
        .output()
        .unwrap();

    assert_eq!(
        output_empty.status.code(),
        Some(0),
        "empty SHELL must not cause non-zero exit when upstream matches: {output_empty:?}"
    );
    let text_empty = String::from_utf8_lossy(&output_empty.stdout);
    assert!(text_empty.contains("Hyprland"));
    assert!(!text_empty.contains("Result:\n  Could not inspect Shell input."));

    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_with_unpredicted_terminal_bytes_exposes_tmux_candidate() {
    let base = temp_dir("tmux-byte-test");
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let tmux = bin.join("tmux");
    fs::write(
        &tmux,
        "#!/bin/sh\ncase \"$*\" in *list-keys*root*) printf '%s\\n' 'bind-key -T root M-Enter split-window -v' ;; *list-keys*) printf '%s\\n' '' ;; *display-message*) printf '%s\\n' 'root' ;; *) exit 0 ;; esac\n",
    )
    .unwrap();
    let ghostty = bin.join("ghostty");
    fs::write(&ghostty, "#!/bin/sh\nexit 0\n").unwrap();
    for cmd in [&tmux, &ghostty] {
        let mut perms = fs::metadata(cmd).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(cmd, perms).unwrap();
    }

    let output = binary()
        .args(["alt+return", "--verbose"])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), env::var("PATH").unwrap_or_default()),
        )
        .env("TMUX", "/tmp/mock-tmux,1,0")
        .env("TMUX_PANE", "%0")
        .env("TERM_PROGRAM", "ghostty")
        .output()
        .unwrap();

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(
            "tmux has a candidate binding for ALT + RETURN in the root table, but terminal byte delivery could not be verified."
        ),
        "tmux candidate must be exposed when bytes are unpredicted; output:\n{text}"
    );
    assert!(
        !text.contains("No inspected layer handles ALT + RETURN."),
        "must not falsely claim no layer handles the key"
    );

    let _ = fs::remove_dir_all(base);
}

#[cfg(unix)]
#[test]
fn inspect_pid_on_non_editor_tui_does_not_fall_through_to_shell() {
    // Start a background process named sleep (simulating a TUI ancestor)
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep must spawn");
    let pid = child.id();
    let output = binary()
        .args(["inspect", "--pid", &pid.to_string(), "ctrl+w", "--verbose"])
        .env("SHELL", "/bin/bash")
        .env(
            "WHYKEY_READLINE_BINDINGS",
            "unix-word-rubout can be found on \"\\C-w\".\n",
        )
        .output()
        .unwrap();

    let _ = child.kill();
    let _ = child.wait();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("Bash / Readline"),
        "a non-shell application target must not fall through to the parent shell; output:\n{text}"
    );
    assert!(
        text.contains("selected process has no dedicated shortcut inspection adapter"),
        "unrecognized application target must report unverified adapter status"
    );
}

#[cfg(unix)]
#[test]
fn inspect_pid_on_shell_inspects_shell_directly() {
    let mut bash = std::process::Command::new("bash")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .expect("bash must spawn");
    let bash_pid = bash.id();

    let output = binary()
        .args([
            "inspect",
            "--pid",
            &bash_pid.to_string(),
            "ctrl+r",
            "--verbose",
        ])
        .env("SHELL", "/bin/bash")
        .env(
            "WHYKEY_READLINE_BINDINGS",
            "reverse-search-history can be found on \"\\C-r\".\n",
        )
        .output()
        .unwrap();

    let _ = bash.kill();
    let _ = bash.wait();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("Bash / Readline"),
        "a shell targeted via --pid must be inspected by the shell layer; output:\n{text}"
    );
    assert!(
        text.contains("reverse-search-history"),
        "readline binding must be reported for shell target"
    );
}
