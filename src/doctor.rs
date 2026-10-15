//! Shared, read-only doctor cause taxonomy.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cause {
    Available,
    NotInstalled,
    PermissionMissing,
    IpcUnreachable,
    ApiDoesNotExpose,
}

impl Cause {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NotInstalled => "not-installed",
            Self::PermissionMissing => "permission-missing",
            Self::IpcUnreachable => "ipc-unreachable",
            Self::ApiDoesNotExpose => "api-does-not-expose",
        }
    }

    pub const fn next_check(self) -> &'static str {
        match self {
            Self::Available => "no further check is needed",
            Self::NotInstalled => "install the integration command or compositor package",
            Self::PermissionMissing => {
                "check device/socket permissions without escalating automatically"
            }
            Self::IpcUnreachable => "check the compositor socket and session environment",
            Self::ApiDoesNotExpose => "use a static configuration source or a supported extension",
        }
    }
}

pub fn ipc(applicable: bool, reachable: bool) -> Cause {
    match (applicable, reachable) {
        (false, _) => Cause::NotInstalled,
        (true, true) => Cause::Available,
        (true, false) => Cause::IpcUnreachable,
    }
}

/// What a read-only look at `/dev/input` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvdevScan {
    /// `/dev/input` is missing: this kernel or container exposes no evdev.
    NoInputDirectory,
    /// `/dev/input` exists but cannot be listed.
    InputDirectoryDenied,
    /// `/dev/input` lists no keyboard event devices.
    NoKeyboardDevices,
    /// Keyboard event devices exist; `readable` is whether one can be opened.
    KeyboardDevices { readable: bool },
}

impl EvdevScan {
    /// Classify `input_dir` once the keyboard device scan is known. The
    /// directory is only listed when no keyboard device was found, to tell an
    /// empty directory from a missing or unreadable one.
    pub fn classify(input_dir: &std::path::Path, keyboard_devices: bool, readable: bool) -> Self {
        if keyboard_devices {
            return Self::KeyboardDevices { readable };
        }
        match crate::recording::read_dir(input_dir) {
            Ok(_) => Self::NoKeyboardDevices,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Self::InputDirectoryDenied
            }
            Err(_) => Self::NoInputDirectory,
        }
    }

    pub const fn cause(self) -> Cause {
        match self {
            Self::KeyboardDevices { readable: true } => Cause::Available,
            Self::KeyboardDevices { readable: false } | Self::InputDirectoryDenied => {
                Cause::PermissionMissing
            }
            Self::NoInputDirectory => Cause::NotInstalled,
            // The input subsystem is present but exposes no keyboard to read.
            Self::NoKeyboardDevices => Cause::ApiDoesNotExpose,
        }
    }

    /// What was found, in words precise enough to act on.
    pub const fn detail(self) -> &'static str {
        match self {
            Self::KeyboardDevices { readable: true } => {
                "a keyboard event device can be opened read-only"
            }
            Self::KeyboardDevices { readable: false } => {
                "keyboard event devices exist under /dev/input, but none can be opened read-only"
            }
            Self::InputDirectoryDenied => "/dev/input exists but cannot be listed",
            Self::NoInputDirectory => "/dev/input does not exist, so no evdev devices are exposed",
            Self::NoKeyboardDevices => "/dev/input lists no keyboard event devices",
        }
    }

    /// The step that would make `--evdev` work, or `None` when it already can.
    /// For unreadable keyboards, [`evdev_acl_command`] names the exact nodes.
    pub const fn next_step(self) -> Option<&'static str> {
        match self {
            Self::KeyboardDevices { readable: true } => None,
            Self::KeyboardDevices { readable: false } => Some(
                "grant your user temporary read access to the keyboard event devices with `sudo setfacl -m u:$USER:r /dev/input/eventN`; joining the input group instead grants permanent access to every input device",
            ),
            Self::InputDirectoryDenied => {
                Some("grant read access to /dev/input and its event devices before using --evdev")
            }
            Self::NoInputDirectory | Self::NoKeyboardDevices => Some(
                "no keyboard is visible to evdev here; use native compositor or terminal capture instead of --evdev",
            ),
        }
    }
}

/// A least-privilege, temporary grant for the keyboards whykey found but
/// cannot open: a read ACL for the current user on exactly those nodes.
/// Device nodes live on devtmpfs, so the ACL disappears when the device is
/// replugged or the system reboots. `None` when no keyboard is unreadable.
pub fn evdev_acl_command(devices: &[crate::listen::EvdevDeviceInfo]) -> Option<String> {
    let (grant, undo) = evdev_acl_commands(devices)?;
    Some(format!(
        "for temporary read access to the detected keyboards run `{grant}`; the grant is lost when the device is replugged or the system reboots (undo it sooner with `{undo}`)"
    ))
}

/// The grant and its undo as two runnable commands, for human output.
pub fn evdev_acl_commands(devices: &[crate::listen::EvdevDeviceInfo]) -> Option<(String, String)> {
    let nodes = devices
        .iter()
        .filter(|device| device.keyboard == Some(true) && !device.readable)
        .map(|device| format!("'{}'", device.path.replace('\'', r"'\''")))
        .collect::<Vec<_>>();
    if nodes.is_empty() {
        return None;
    }
    let nodes = nodes.join(" ");
    Some((
        format!("sudo setfacl -m u:\"$USER\":r {nodes}"),
        format!("sudo setfacl -x u:\"$USER\" {nodes}"),
    ))
}

pub const fn api_unavailable() -> Cause {
    Cause::ApiDoesNotExpose
}

/// One way whykey can watch a key right now, in the order `whykey listen`
/// tries them, followed by the opt-in taps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservationTap {
    pub id: &'static str,
    pub name: &'static str,
    pub available: bool,
    pub detail: String,
    /// The exact step that makes an unavailable tap usable, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable: Option<String>,
}

/// What doctor found about each tap; gathered by the caller so the
/// classification stays a pure function.
#[derive(Debug, Clone, Default)]
pub struct TapInputs {
    /// The native capture backend that accepted a connection.
    pub native: Option<&'static str>,
    /// `WAYLAND_DISPLAY` when its socket exists.
    pub wayland: Option<String>,
    /// `DISPLAY` when set.
    pub x11: Option<String>,
    /// The X server is XWayland inside a Wayland session.
    pub xwayland: bool,
    /// Remote session without a local display server.
    pub remote: bool,
    pub tty: bool,
    pub evdev_readable: bool,
    pub evdev_detail: &'static str,
    /// The exact step that makes evdev readable (temporary ACL command).
    pub evdev_enable: Option<String>,
}

pub fn observation_taps(inputs: &TapInputs) -> Vec<ObservationTap> {
    let tap = |id, name, available, detail: String, enable: Option<String>| ObservationTap {
        id,
        name,
        available,
        detail,
        enable,
    };
    let displays = [
        inputs
            .wayland
            .as_ref()
            .map(|display| format!("Wayland ({display})")),
        inputs
            .x11
            .as_ref()
            .map(|display| format!("X11 ({display})")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let (grab, grab_detail) = match (&inputs.x11, inputs.remote) {
        (None, _) => (false, "DISPLAY is not set".to_owned()),
        (Some(_), true) => (
            false,
            "remote session: DISPLAY does not name a local X server".to_owned(),
        ),
        (Some(display), false) if inputs.xwayland => (
            true,
            format!("asks XWayland on {display}; covers X11 clients' grabs only"),
        ),
        (Some(display), false) => (
            true,
            format!("asks the X server on {display} which shortcuts a client has grabbed"),
        ),
    };
    vec![
        tap(
            "native-compositor",
            "native compositor capture",
            inputs.native.is_some(),
            match inputs.native {
                Some(backend) => format!("{backend} IPC accepted the capture connection"),
                None => "no Hyprland or Sway capture IPC is reachable".into(),
            },
            None,
        ),
        tap(
            "probe-window",
            "probe window",
            !displays.is_empty(),
            if displays.is_empty() {
                "no Wayland socket (WAYLAND_DISPLAY) or X11 DISPLAY".into()
            } else {
                format!(
                    "{} available; `whykey listen --window` opens it",
                    displays.join(" and ")
                )
            },
            None,
        ),
        tap("x11-grab", "X11 grab probe", grab, grab_detail, None),
        tap(
            "terminal",
            "terminal capture",
            inputs.tty,
            if inputs.tty {
                "controlling /dev/tty is readable".into()
            } else {
                "no controlling terminal".into()
            },
            None,
        ),
        tap(
            "evdev",
            "evdev (`whykey listen --evdev`)",
            inputs.evdev_readable,
            inputs.evdev_detail.to_owned(),
            (!inputs.evdev_readable)
                .then(|| inputs.evdev_enable.clone())
                .flatten(),
        ),
    ]
}

/// Where a layer's answer comes from in this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// The running program is asked.
    LiveQuery,
    /// Configuration files (or built-in defaults) are parsed.
    ConfigFile,
    /// Nothing answers for this layer.
    None,
}

impl SourceKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::LiveQuery => "live query",
            Self::ConfigFile => "config file",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayerSource {
    pub layer: &'static str,
    pub source: SourceKind,
    pub detail: String,
}

impl LayerSource {
    pub fn new(layer: &'static str, source: SourceKind, detail: impl Into<String>) -> Self {
        Self {
            layer,
            source,
            detail: detail.into(),
        }
    }
}

/// Desktop adapters that ask the running program (compositor IPC,
/// gsettings, xfconf, kglobalaccel D-Bus, herbstclient) once reachable;
/// every other adapter parses configuration files.
const LIVE_DESKTOPS: &[&str] = &[
    "hyprland",
    "sway",
    "i3",
    "kde",
    "xfce",
    "cinnamon",
    "mate",
    "gnome",
    "herbstluftwm",
];

/// The source of a desktop adapter's answer given whether its source is
/// reachable. KDE falls back to kglobalshortcutsrc when kglobalaccel is not
/// running.
pub fn desktop_source(id: &str, reachable: bool) -> SourceKind {
    match (LIVE_DESKTOPS.contains(&id), reachable) {
        (true, true) => SourceKind::LiveQuery,
        (true, false) if id == "kde" => SourceKind::ConfigFile,
        (false, true) => SourceKind::ConfigFile,
        (_, false) => SourceKind::None,
    }
}

/// One row of the human `doctor` report.
#[derive(Debug, Clone)]
pub struct Row {
    pub state: Option<crate::style::Check>,
    pub name: String,
    /// One-line reason; wrapped under the name column when it is long.
    pub reason: String,
    /// A command that fixes the row, on its own line.
    pub fix: Option<String>,
    /// A command that reverts `fix`, on its own line.
    pub undo: Option<String>,
    /// Extra detail, dimmed, under the row.
    pub notes: Vec<String>,
}

impl Row {
    pub fn new(state: crate::style::Check, name: impl Into<String>) -> Self {
        Self {
            state: Some(state),
            name: name.into(),
            reason: String::new(),
            fix: None,
            undo: None,
            notes: Vec::new(),
        }
    }

    /// A row with no state glyph (layer sources).
    pub fn plain(name: impl Into<String>) -> Self {
        Self {
            state: None,
            ..Self::new(crate::style::Check::Ok, name)
        }
    }

    pub fn reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    pub fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    pub fn undo(mut self, undo: impl Into<String>) -> Self {
        self.undo = Some(undo.into());
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Group {
    pub title: String,
    pub rows: Vec<Row>,
}

impl Group {
    pub fn new(title: impl Into<String>, rows: Vec<Row>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }
}

/// Render the `doctor` report: a header with the session context on the
/// right, titled groups of state/name/reason rows, `fix` and `undo` on their
/// own lines, and a one-line summary of the states.
pub fn render_report(
    context: &str,
    groups: &[Group],
    options: crate::style::RenderOptions,
) -> String {
    use crate::style::{Check, display_width, wrap_hanging};

    let panel = options.width.min(80);
    let mut output = String::new();
    let title = "whykey doctor";
    let left = display_width(title);
    let right = display_width(context);
    output.push_str(&options.bold(title));
    if !context.is_empty() && left + 2 + right <= panel {
        output.push_str(&" ".repeat(panel - left - right));
        output.push_str(&options.muted(context));
    }
    output.push('\n');

    let name_width = groups
        .iter()
        .flat_map(|group| &group.rows)
        .map(|row| display_width(&row.name))
        .max()
        .unwrap_or(0)
        .min(32);
    let mut counts = [0usize; 4];
    for group in groups.iter().filter(|group| !group.rows.is_empty()) {
        output.push('\n');
        output.push_str(&options.heading(&group.title));
        output.push('\n');
        for row in &group.rows {
            if let Some(state) = row.state {
                counts[state as usize] += 1;
            }
            let glyph = row
                .state
                .map_or_else(|| " ".to_owned(), |state| options.check(state));
            let name_pad = " ".repeat((name_width + 2).saturating_sub(display_width(&row.name)));
            let prefix = format!("  {glyph}  {}{name_pad}", row.name);
            let prefix_width = 5 + name_width + 2;
            let indent = " ".repeat(prefix_width);
            let limit = panel.saturating_sub(prefix_width).max(20);
            let reason = wrap_hanging(&row.reason, limit, "", "");
            if row.reason.trim().is_empty() {
                output.push_str(prefix.trim_end());
                output.push('\n');
            }
            for (index, line) in reason
                .iter()
                .enumerate()
                .filter(|_| !row.reason.trim().is_empty())
            {
                if index == 0 {
                    output.push_str(&format!("{prefix}{line}\n"));
                } else {
                    output.push_str(&format!("{indent}{line}\n"));
                }
            }
            let sub = " ".repeat(prefix_width.min(7));
            for (label, command) in [("fix ", &row.fix), ("undo", &row.undo)] {
                if let Some(command) = command {
                    output.push_str(&format!("{sub}{} {command}\n", options.accent(label)));
                }
            }
            for note in &row.notes {
                output.push_str(&format!("{sub}{}\n", options.muted(note)));
            }
        }
    }
    let total = |check: Check| counts[check as usize];
    let mut parts = vec![format!("{} ok", total(Check::Ok))];
    parts.push(format!("{} unavailable", total(Check::Fail)));
    if total(Check::Unknown) > 0 {
        parts.push(format!("{} unknown", total(Check::Unknown)));
    }
    parts.push(format!("{} not applicable", total(Check::Skip)));
    let separator = if options.ascii { " | " } else { " \u{b7} " };
    output.push_str(&format!(
        "\n{}  {}\n",
        options.bold("Summary"),
        parts.join(separator)
    ));
    options.fold(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cause_taxonomy_has_actionable_next_checks() {
        for cause in [
            Cause::NotInstalled,
            Cause::PermissionMissing,
            Cause::IpcUnreachable,
            Cause::ApiDoesNotExpose,
        ] {
            assert!(!cause.label().is_empty());
            assert!(!cause.next_check().is_empty());
        }
    }

    #[test]
    fn maps_each_integration_state_deterministically() {
        assert_eq!(ipc(false, false), Cause::NotInstalled);
        assert_eq!(ipc(true, false), Cause::IpcUnreachable);
        assert_eq!(ipc(true, true), Cause::Available);
        assert_eq!(
            EvdevScan::KeyboardDevices { readable: false }.cause(),
            Cause::PermissionMissing
        );
        assert_eq!(api_unavailable(), Cause::ApiDoesNotExpose);
    }

    #[test]
    fn missing_keyboards_are_not_reported_as_missing_permissions_or_software() {
        let base = std::env::temp_dir().join(format!("whykey-evdev-scan-{}", std::process::id()));
        let input = base.join("input");
        std::fs::create_dir_all(&input).unwrap();

        let empty = EvdevScan::classify(&input, false, false);
        assert_eq!(empty, EvdevScan::NoKeyboardDevices);
        assert_eq!(empty.cause(), Cause::ApiDoesNotExpose);
        assert!(!empty.next_step().unwrap().contains("grant"));

        let missing = EvdevScan::classify(&base.join("absent"), false, false);
        assert_eq!(missing, EvdevScan::NoInputDirectory);
        assert_eq!(missing.cause(), Cause::NotInstalled);

        assert_eq!(
            EvdevScan::classify(&input, true, false).cause(),
            Cause::PermissionMissing
        );
        assert_eq!(
            EvdevScan::classify(&input, true, true).cause(),
            Cause::Available
        );
        assert_eq!(EvdevScan::classify(&input, true, true).next_step(), None);
        assert_eq!(
            EvdevScan::InputDirectoryDenied.cause(),
            Cause::PermissionMissing
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn acl_command_names_only_unreadable_keyboards_and_is_temporary() {
        let device = |path: &str, keyboard, readable| crate::listen::EvdevDeviceInfo {
            path: path.into(),
            name: "kbd".into(),
            keyboard,
            readable,
            error: None,
        };
        assert_eq!(
            evdev_acl_command(&[device("/dev/input/event3", Some(true), true)]),
            None
        );
        let command = evdev_acl_command(&[
            device("/dev/input/event3", Some(true), false),
            device("/dev/input/event4", None, false),
            device("/dev/input/event5", Some(true), true),
            device("/dev/input/it's", Some(true), false),
        ])
        .unwrap();
        assert!(
            command.contains(
                "`sudo setfacl -m u:\"$USER\":r '/dev/input/event3' '/dev/input/it'\\''s'`"
            )
        );
        assert!(!command.contains("event4") && !command.contains("event5"));
        assert!(command.contains("replugged") && command.contains("reboots"));
    }

    #[test]
    fn observation_taps_follow_the_listen_chain_and_say_how_to_enable_evdev() {
        let taps = observation_taps(&TapInputs {
            wayland: Some("wayland-1".into()),
            x11: Some(":0".into()),
            xwayland: true,
            tty: true,
            evdev_detail: EvdevScan::KeyboardDevices { readable: false }.detail(),
            evdev_enable: Some("run `sudo setfacl -m u:\"$USER\":r '/dev/input/event3'`".into()),
            ..TapInputs::default()
        });
        let ids = taps.iter().map(|tap| tap.id).collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "native-compositor",
                "probe-window",
                "x11-grab",
                "terminal",
                "evdev"
            ]
        );
        assert!(!taps[0].available);
        assert!(taps[1].available);
        assert!(taps[1].detail.contains("Wayland (wayland-1) and X11 (:0)"));
        assert!(taps[2].available && taps[2].detail.contains("X11 clients' grabs only"));
        assert!(taps[3].available);
        assert!(!taps[4].available);
        assert!(taps[4].enable.as_deref().unwrap().contains("sudo setfacl"));

        let headless = observation_taps(&TapInputs {
            native: Some("Sway"),
            x11: Some(":0".into()),
            remote: true,
            evdev_readable: true,
            evdev_enable: Some("unused".into()),
            ..TapInputs::default()
        });
        assert!(headless[0].available);
        assert!(headless[1].available && headless[1].detail.contains("X11 (:0)"));
        assert!(!headless[2].available);
        assert!(!headless[3].available);
        assert!(headless[4].available && headless[4].enable.is_none());
    }

    #[test]
    fn desktop_sources_distinguish_live_queries_from_files() {
        assert_eq!(desktop_source("hyprland", true), SourceKind::LiveQuery);
        assert_eq!(desktop_source("gnome", true), SourceKind::LiveQuery);
        assert_eq!(desktop_source("sway", false), SourceKind::None);
        assert_eq!(desktop_source("kde", false), SourceKind::ConfigFile);
        assert_eq!(desktop_source("openbox", true), SourceKind::ConfigFile);
        assert_eq!(desktop_source("openbox", false), SourceKind::None);
    }

    #[test]
    fn doctor_report_aligns_states_and_puts_fix_and_undo_on_their_own_lines() {
        use crate::style::{Check, RenderOptions};
        let groups = vec![
            Group::new(
                "Capture",
                vec![
                    Row::new(Check::Ok, "Hyprland native capture"),
                    Row::new(Check::Fail, "controlling TTY").reason("run whykey inside a terminal"),
                    Row::new(Check::Fail, "evdev keyboards")
                        .reason("no read access to 2 devices")
                        .fix("sudo setfacl -m u:\"$USER\":r '/dev/input/event2'")
                        .undo("sudo setfacl -x u:\"$USER\" '/dev/input/event2'")
                        .note("/dev/input/event2 - kbd (Permission denied)"),
                ],
            ),
            Group::new(
                "Live readers",
                vec![
                    Row::new(Check::Skip, "kglobalaccel").reason("not running"),
                    Row::new(Check::Unknown, "portal").reason("malformed answer"),
                ],
            ),
            Group::new(
                "Layer sources",
                vec![Row::plain("shell").reason("config file - bash")],
            ),
        ];
        let output = render_report(
            "Hyprland - Wayland - bash",
            &groups,
            RenderOptions::plain(false),
        );
        crate::style::assert_golden("doctor_report_layout", &output);
    }
}
