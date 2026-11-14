//! Built-in key bindings of the terminals whose key route whykey reads from
//! a configuration file.
//!
//! A terminal's own shortcuts act before the program on its PTY sees the key,
//! and a configuration file lists only the changes. Without these tables a key
//! such as Ctrl+Shift+C looked "unbound" in a terminal that copies on it.
//!
//! Each table is transcribed from the project's own source for the pinned
//! version below, Linux only (macOS-only entries are left out), and only for
//! the keyboard mode a shell prompt is in. Modal bindings (Alacritty's Vi and
//! search modes, kitty's `--mode` maps) are not active at a prompt and are not
//! listed. A newer or older release may differ, which is why every match found
//! here carries the `Default` evidence tier.
//!
//! * kitty 0.50.0: `kitty/options/definition.py` (`map(...)` entries).
//!   Checked against the binary's own `definition.iter_all_maps()`
//!   (`lab/defaults/kitty-*.txt`, `python3 tools/dump_defaults.py dump kitty`).
//!   The lab image carries kitty 0.47.1, not 0.50.0, so the check proves the
//!   table is a faithful 0.47.1 default set; a difference introduced by
//!   0.48 to 0.50 would need that binary in the lab.
//! * Alacritty 0.17.0: `alacritty/src/config/bindings.rs` (`default_key_bindings`,
//!   `common_keybindings`). Not checked against the binary: Alacritty has no
//!   command that lists its built-in bindings (they are compiled in; `migrate`
//!   and `--print-events` do not print them), so this table is the one that
//!   can drift unseen.
//! * foot 1.28.0: `config.c` (`add_default_key_bindings`) and `foot.ini(5)`.
//!   Checked against the `[key-bindings]` block of the `foot.ini` that the
//!   1.28.0 package installs (`lab/defaults/foot-*.txt`, dumped with `--source
//!   host`: the lab image has foot 1.27.0).
//! * Konsole 26.08.3: `MainWindow.cpp`, `ViewManager.cpp` and
//!   `SessionController.cpp` (`setDefaultShortcut`, where `Konsole::ACCEL` is
//!   Ctrl+Shift on Linux). Not checked against the binary: the shortcuts are
//!   set in C++, there is no listing command and Konsole does not start
//!   without a display.

use super::{Binding, TerminalKind};
use crate::key::KeyCombo;

pub(super) const KITTY_VERSION: &str = "0.50.0";
pub(super) const ALACRITTY_VERSION: &str = "0.17.0";
pub(super) const FOOT_VERSION: &str = "1.28.0";
pub(super) const KONSOLE_VERSION: &str = "26.08.3";

/// The version the defaults of a terminal were transcribed for.
pub(super) fn version(kind: TerminalKind) -> Option<&'static str> {
    match kind {
        TerminalKind::Kitty => Some(KITTY_VERSION),
        TerminalKind::Alacritty => Some(ALACRITTY_VERSION),
        TerminalKind::Foot => Some(FOOT_VERSION),
        TerminalKind::Konsole => Some(KONSOLE_VERSION),
        TerminalKind::WezTerm => None,
    }
}

/// `kitty_mod` is substituted by the caller (it is Ctrl+Shift unless the
/// configuration changes it). The two `key_sequence_prefix` rows are the first
/// key of kitty's multi-key defaults: `kitty_mod+p>f`, `>shift+f`, `>c`, `>d`,
/// `>l`, `>w`, `>h`, `>n`, `>y` (the hints kitten) and `kitty_mod+a>m`, `>l`,
/// `>1`, `>d` (background opacity).
const KITTY: &[(&str, &str)] = &[
    ("kitty_mod+c", "copy_to_clipboard"),
    ("kitty_mod+v", "paste_from_clipboard"),
    ("kitty_mod+s", "paste_from_selection"),
    ("shift+insert", "paste_from_selection"),
    ("kitty_mod+o", "pass_selection_to_program"),
    ("kitty_mod+up", "scroll_line_up smooth"),
    ("kitty_mod+k", "scroll_line_up smooth"),
    ("kitty_mod+down", "scroll_line_down smooth"),
    ("kitty_mod+j", "scroll_line_down smooth"),
    ("kitty_mod+page_up", "scroll_page_up"),
    ("kitty_mod+page_down", "scroll_page_down"),
    ("kitty_mod+home", "scroll_home"),
    ("kitty_mod+end", "scroll_end"),
    ("kitty_mod+z", "scroll_to_prompt -1"),
    ("kitty_mod+x", "scroll_to_prompt 1"),
    ("kitty_mod+h", "show_scrollback"),
    ("kitty_mod+g", "show_last_command_output"),
    ("kitty_mod+/", "search_scrollback"),
    ("kitty_mod+enter", "new_window"),
    ("kitty_mod+n", "new_os_window"),
    ("kitty_mod+w", "close_window"),
    ("kitty_mod+]", "next_window"),
    ("kitty_mod+[", "previous_window"),
    ("kitty_mod+f", "move_window_forward"),
    ("kitty_mod+b", "move_window_backward"),
    ("kitty_mod+`", "move_window_to_top"),
    ("kitty_mod+r", "start_resizing_window"),
    ("kitty_mod+1", "first_window"),
    ("kitty_mod+2", "second_window"),
    ("kitty_mod+3", "third_window"),
    ("kitty_mod+4", "fourth_window"),
    ("kitty_mod+5", "fifth_window"),
    ("kitty_mod+6", "sixth_window"),
    ("kitty_mod+7", "seventh_window"),
    ("kitty_mod+8", "eighth_window"),
    ("kitty_mod+9", "ninth_window"),
    ("kitty_mod+0", "tenth_window"),
    ("kitty_mod+f7", "focus_visible_window"),
    ("kitty_mod+f8", "swap_with_window"),
    ("kitty_mod+right", "next_tab"),
    ("ctrl+tab", "next_tab"),
    ("kitty_mod+left", "previous_tab"),
    ("ctrl+shift+tab", "previous_tab"),
    ("kitty_mod+t", "new_tab"),
    ("kitty_mod+q", "close_tab"),
    ("kitty_mod+.", "move_tab_forward"),
    ("kitty_mod+,", "move_tab_backward"),
    ("kitty_mod+alt+t", "set_tab_title"),
    ("kitty_mod+l", "next_layout"),
    ("kitty_mod+equal", "change_font_size all +2.0"),
    ("kitty_mod+plus", "change_font_size all +2.0"),
    ("kitty_mod+kp_add", "change_font_size all +2.0"),
    ("kitty_mod+minus", "change_font_size all -2.0"),
    ("kitty_mod+kp_subtract", "change_font_size all -2.0"),
    ("kitty_mod+backspace", "change_font_size all 0"),
    ("kitty_mod+e", "open_url_with_hints"),
    (
        "kitty_mod+p",
        "key_sequence_prefix (hints kitten: p>f, p>shift+f, p>c, p>d, p>l, p>w, p>h, p>n, p>y)",
    ),
    ("kitty_mod+f1", "show_kitty_doc overview"),
    ("kitty_mod+f3", "command_palette"),
    ("kitty_mod+f11", "toggle_fullscreen"),
    ("kitty_mod+f10", "toggle_maximized"),
    ("kitty_mod+u", "kitten unicode_input"),
    ("kitty_mod+f2", "edit_config_file"),
    ("kitty_mod+escape", "kitty_shell window"),
    (
        "kitty_mod+a",
        "key_sequence_prefix (background opacity: a>m, a>l, a>1, a>d)",
    ),
    ("kitty_mod+delete", "clear_terminal reset active"),
    ("kitty_mod+f5", "load_config_file"),
    ("kitty_mod+f6", "debug_config"),
];

/// `(action, keys)`: foot binds an action to a list of keys, and a
/// `[key-bindings]` entry for the action replaces every key listed here.
pub(super) const FOOT: &[(&str, &[&str])] = &[
    ("scrollback-up-page", &["Shift+Page_Up", "Shift+KP_Page_Up"]),
    (
        "scrollback-down-page",
        &["Shift+Page_Down", "Shift+KP_Page_Down"],
    ),
    ("clipboard-copy", &["Control+Shift+c", "XF86Copy"]),
    ("clipboard-paste", &["Control+Shift+v", "XF86Paste"]),
    ("primary-paste", &["Shift+Insert"]),
    ("search-start", &["Control+Shift+r"]),
    (
        "font-increase",
        &["Control+plus", "Control+equal", "Control+KP_Add"],
    ),
    ("font-decrease", &["Control+minus", "Control+KP_Subtract"]),
    ("font-reset", &["Control+0", "Control+KP_0"]),
    ("spawn-terminal", &["Control+Shift+n"]),
    ("show-urls-launch", &["Control+Shift+o"]),
    ("unicode-input", &["Control+Shift+u"]),
    ("prompt-prev", &["Control+Shift+z"]),
    ("prompt-next", &["Control+Shift+x"]),
];

/// `(keys, action, mode_dependent)`. Alacritty's default `ReceiveChar` rows
/// (Ctrl+L) forward the key; the scroll keys act only off the alternate screen.
const ALACRITTY: &[(&str, &str, bool)] = &[
    ("ctrl+shift+c", "Copy", false),
    ("ctrl+shift+v", "Paste", false),
    ("ctrl+shift+f", "SearchForward", false),
    ("ctrl+shift+b", "SearchBackward", false),
    ("shift+insert", "PasteSelection", false),
    ("ctrl+0", "ResetFontSize", false),
    ("ctrl+equal", "IncreaseFontSize", false),
    ("ctrl+plus", "IncreaseFontSize", false),
    ("ctrl+minus", "DecreaseFontSize", false),
    ("ctrl+kp_add", "IncreaseFontSize", false),
    ("ctrl+kp_subtract", "DecreaseFontSize", false),
    ("ctrl+shift+space", "ToggleViMode", false),
    ("ctrl+l", "ClearLogNotice, ReceiveChar", false),
    ("shift+home", "ScrollToTop", true),
    ("shift+end", "ScrollToBottom", true),
    ("shift+page_up", "ScrollPageUp", true),
    ("shift+page_down", "ScrollPageDown", true),
];

/// Konsole's application shortcuts (`ACCEL` is Ctrl+Shift). The keytab, not
/// these, decides how a key is encoded for the PTY.
const KONSOLE: &[(&str, &str)] = &[
    ("ctrl+shift+t", "new-tab"),
    ("ctrl+shift+n", "new-window"),
    ("ctrl+shift+q", "close-window"),
    ("ctrl+shift+m", "show menubar"),
    ("f11", "full screen"),
    ("ctrl+shift+f10", "activate-menu"),
    ("shift+f10", "hamburger menu"),
    ("ctrl+shift+w", "close-session"),
    ("ctrl+shift+c", "copy"),
    ("ctrl+insert", "copy"),
    ("ctrl+shift+v", "paste"),
    ("shift+insert", "paste"),
    ("ctrl+shift+insert", "paste-selection"),
    ("ctrl+shift+d", "select-mode"),
    ("ctrl+shift+p", "print screen"),
    ("ctrl+shift+k", "clear-history-and-reset"),
    ("ctrl+shift+f", "find"),
    ("f3", "find next"),
    ("shift+f3", "find previous"),
    ("ctrl+alt+n", "next-profile"),
    ("ctrl+alt+m", "prev-profile"),
    ("ctrl+alt+s", "rename-session"),
    ("ctrl+alt+u", "zmodem-upload"),
    ("ctrl+shift+.", "copy input to selected tabs"),
    ("ctrl+shift+/", "copy input to none"),
    ("ctrl+shift+r", "monitor session"),
    ("ctrl+shift+a", "monitor session"),
    ("ctrl+shift+i", "monitor session"),
    ("ctrl+plus", "enlarge-font"),
    ("ctrl+equal", "enlarge-font"),
    ("ctrl+minus", "shrink-font"),
    ("ctrl+alt+0", "reset-font-size"),
    ("ctrl+shift+]", "expand-active-view"),
    ("ctrl+shift+[", "shrink-active-view"),
    ("ctrl+shift+h", "detach-view"),
    ("shift+right", "next-tab"),
    ("ctrl+page_down", "next-tab"),
    ("shift+left", "previous-tab"),
    ("ctrl+page_up", "previous-tab"),
    ("ctrl+shift+up", "focus-view-above"),
    ("ctrl+shift+down", "focus-view-below"),
    ("ctrl+shift+left", "focus-view-left"),
    ("ctrl+shift+right", "focus-view-right"),
    ("ctrl+f11", "focus-view-next"),
    ("ctrl+shift+f11", "focus-view-prev"),
    ("ctrl+tab", "last-used-tab"),
    ("ctrl+shift+tab", "last-used-tab-reverse"),
    ("ctrl+shift+e", "toggle-maximize-current-view"),
    ("ctrl+shift+z", "toggle-zoom-current-view"),
    ("ctrl+alt+right", "move-tab-to-right"),
    ("ctrl+alt+left", "move-tab-to-left"),
    ("ctrl+alt+]", "semantic-setup-bash"),
    ("ctrl+alt+[", "toggle-semantic-hints"),
    ("ctrl+alt+\\", "toggle-line-numbers"),
    ("ctrl+shift+\\", "toggle-line-numbers"),
    ("alt+1", "switch-to-tab-1"),
    ("alt+2", "switch-to-tab-2"),
    ("alt+3", "switch-to-tab-3"),
    ("alt+4", "switch-to-tab-4"),
    ("alt+5", "switch-to-tab-5"),
    ("alt+6", "switch-to-tab-6"),
    ("alt+7", "switch-to-tab-7"),
    ("alt+8", "switch-to-tab-8"),
    ("alt+9", "switch-to-tab-9"),
    ("alt+0", "switch-to-tab-10"),
];

fn default_binding(trigger: KeyCombo, action: &str, kind: TerminalKind, version: &str) -> Binding {
    let description = format!("{trigger} {action} ({kind} {version} default)");
    let mut binding = Binding::new(trigger, action, None, description);
    binding.origin = super::Origin::Default;
    binding
}

/// kitty's default shortcuts with `kitty_mod` replaced by `modifiers`
/// (`ctrl+shift` unless the configuration sets it).
pub(super) fn kitty(modifiers: &str) -> Vec<Binding> {
    KITTY
        .iter()
        .filter_map(|(keys, action)| {
            let keys = super::kitty::substitute_kitty_mod(keys, modifiers);
            let trigger = keys.parse().ok()?;
            Some(default_binding(
                trigger,
                action,
                TerminalKind::Kitty,
                KITTY_VERSION,
            ))
        })
        .collect()
}

pub(super) fn alacritty() -> Vec<Binding> {
    ALACRITTY
        .iter()
        .filter_map(|(keys, action, mode_dependent)| {
            let mut binding = default_binding(
                keys.parse().ok()?,
                action,
                TerminalKind::Alacritty,
                ALACRITTY_VERSION,
            );
            if *mode_dependent {
                binding.uncertain = Some(
                    "Alacritty acts on it only off the alternate screen; a full-screen program receives it".into(),
                );
            }
            Some(binding)
        })
        .collect()
}

/// foot's default bindings, minus those of every action in `replaced` (foot
/// drops an action's default keys as soon as the configuration mentions the
/// action, even as `none`).
pub(super) fn foot(replaced: &[String]) -> Vec<Binding> {
    FOOT.iter()
        .filter(|(action, _)| !replaced.iter().any(|name| name == action))
        .flat_map(|(action, keys)| {
            keys.iter().filter_map(move |keys| {
                Some(default_binding(
                    keys.parse().ok()?,
                    action,
                    TerminalKind::Foot,
                    FOOT_VERSION,
                ))
            })
        })
        .collect()
}

pub(super) fn konsole() -> Vec<Binding> {
    KONSOLE
        .iter()
        .filter_map(|(keys, action)| {
            Some(default_binding(
                keys.parse().ok()?,
                action,
                TerminalKind::Konsole,
                KONSOLE_VERSION,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row must parse: a row that does not is silently absent from the
    /// answer, which is the failure these tables exist to prevent.
    fn own(rows: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
        rows.collect()
    }

    /// The foot table is the `[key-bindings]` defaults of the real foot.
    #[test]
    fn foot_defaults_equal_the_dump_of_the_real_foot() {
        let Some(fixture) = crate::defaults_fixtures::load("foot") else {
            return;
        };
        assert_eq!(fixture.version, FOOT_VERSION);
        let embedded = own(FOOT
            .iter()
            .map(|(action, keys)| ((*action).to_owned(), keys.join(" "))));
        crate::defaults_fixtures::assert_same_rows("foot defaults", &embedded, &fixture);
    }

    /// The kitty table is the binary's `map` defaults without the macOS keys
    /// (`cmd`); the multi-key defaults are the two `key_sequence_prefix` rows,
    /// whose description lists the continuations.
    #[test]
    fn kitty_defaults_equal_the_dump_of_the_real_kitty() {
        let Some(fixture) = crate::defaults_fixtures::load("kitty") else {
            return;
        };
        let linux = |key: &str| !key.split('+').any(|part| part == "cmd");
        let (sequences, singles): (Vec<_>, Vec<_>) = fixture
            .rows
            .iter()
            .filter(|(key, _)| linux(key))
            .cloned()
            .partition(|(key, _)| key.contains('>'));
        let (prefixes, plain): (Vec<_>, Vec<_>) = KITTY
            .iter()
            .map(|(key, action)| ((*key).to_owned(), (*action).to_owned()))
            .partition(|(_, action)| action.starts_with("key_sequence_prefix"));
        let single_fixture = crate::defaults_fixtures::Fixture {
            version: fixture.version.clone(),
            rows: singles,
        };
        crate::defaults_fixtures::assert_same_rows("kitty defaults", &plain, &single_fixture);

        // Every continuation the binary has is named by its prefix row, and
        // the prefix rows name nothing else.
        let continuation = |key: &str| {
            let (first, rest) = key.split_once('>').unwrap();
            (
                first.to_owned(),
                format!("{}>{rest}", first.rsplit('+').next().unwrap()),
            )
        };
        let mut real: Vec<_> = sequences.iter().map(|(key, _)| continuation(key)).collect();
        let mut named = Vec::new();
        for (prefix, description) in &prefixes {
            let listed = description
                .split_once(": ")
                .unwrap()
                .1
                .trim_end_matches(')');
            named.extend(listed.split(", ").map(|c| (prefix.clone(), c.to_owned())));
        }
        real.sort();
        named.sort();
        assert_eq!(named, real, "kitty key_sequence_prefix rows vs the binary");
    }

    #[test]
    fn every_default_row_parses_to_a_key() {
        assert_eq!(kitty("ctrl+shift").len(), KITTY.len());
        assert_eq!(alacritty().len(), ALACRITTY.len());
        assert_eq!(
            foot(&[]).len(),
            FOOT.iter().map(|(_, keys)| keys.len()).sum::<usize>()
        );
        assert_eq!(konsole().len(), KONSOLE.len());
    }
}
