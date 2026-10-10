use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::layers::{
    BindingEvidence, BindingScope, EvidenceTier, LayerId, LayerResult, Outcome, SourceLocation,
    TerminalInput, UncertaintyReason, format_bytes, terminal,
};

const LAYER: &str = "Bash / Readline";
const SHELL_INIT_HINT: &str =
    "~/.bashrc `bind` commands (e.g. fzf) may override — load `eval \"$(whykey shell-init bash)\"`";
/// Continuations listed for a prefix key; `\C-x` alone has over a dozen.
const MAX_CONTINUATIONS: usize = 4;

/// Built-in Readline defaults, regenerated from bash 5.3.9(1)-release
/// (GNU Readline 8.3) in the lab image with
/// `env -i HOME=/tmp/e INPUTRC=/dev/null bash --norc --noprofile -i -c 'bind -p'`
/// (`set -o vi` first for the vi-insert table; `bind -s` lists no macros).
/// Only `self-insert` lines are left out: a printable key inserts itself.
/// `\C-x` and `\e` are prefix keys: they are unbound on their own.
const EMACS_DEFAULTS: &[(&str, &str)] = &[
    (r#"\C-g"#, "abort"),
    (r#"\C-x\C-g"#, "abort"),
    (r#"\e\C-g"#, "abort"),
    (r#"\C-j"#, "accept-line"),
    (r#"\C-m"#, "accept-line"),
    (r#"\C-b"#, "backward-char"),
    (r#"\eOD"#, "backward-char"),
    (r#"\e[D"#, "backward-char"),
    (r#"\C-h"#, "backward-delete-char"),
    (r#"\C-?"#, "backward-delete-char"),
    (r#"\C-x\C-?"#, "backward-kill-line"),
    (r#"\e\C-h"#, "backward-kill-word"),
    (r#"\e\C-?"#, "backward-kill-word"),
    (r#"\e[1;3D"#, "backward-word"),
    (r#"\e[1;5D"#, "backward-word"),
    (r#"\eb"#, "backward-word"),
    (r#"\e<"#, "beginning-of-history"),
    (r#"\C-a"#, "beginning-of-line"),
    (r#"\eOH"#, "beginning-of-line"),
    (r#"\e[H"#, "beginning-of-line"),
    (r#"\e[200~"#, "bracketed-paste-begin"),
    (r#"\C-xe"#, "call-last-kbd-macro"),
    (r#"\ec"#, "capitalize-word"),
    (r#"\C-]"#, "character-search"),
    (r#"\e\C-]"#, "character-search-backward"),
    (r#"\e\C-l"#, "clear-display"),
    (r#"\C-l"#, "clear-screen"),
    (r#"\C-i"#, "complete"),
    (r#"\e\e"#, "complete"),
    (r#"\e!"#, "complete-command"),
    (r#"\e/"#, "complete-filename"),
    (r#"\e@"#, "complete-hostname"),
    (r#"\e{"#, "complete-into-braces"),
    (r#"\e~"#, "complete-username"),
    (r#"\e$"#, "complete-variable"),
    (r#"\C-d"#, "delete-char"),
    (r#"\e[3~"#, "delete-char"),
    (r#"\e\\"#, "delete-horizontal-space"),
    (r#"\e-"#, "digit-argument"),
    (r#"\e0"#, "digit-argument"),
    (r#"\e1"#, "digit-argument"),
    (r#"\e2"#, "digit-argument"),
    (r#"\e3"#, "digit-argument"),
    (r#"\e4"#, "digit-argument"),
    (r#"\e5"#, "digit-argument"),
    (r#"\e6"#, "digit-argument"),
    (r#"\e7"#, "digit-argument"),
    (r#"\e8"#, "digit-argument"),
    (r#"\e9"#, "digit-argument"),
    (r#"\C-x\C-v"#, "display-shell-version"),
    (r#"\C-xA"#, "do-lowercase-version"),
    (r#"\C-xB"#, "do-lowercase-version"),
    (r#"\C-xC"#, "do-lowercase-version"),
    (r#"\C-xD"#, "do-lowercase-version"),
    (r#"\C-xE"#, "do-lowercase-version"),
    (r#"\C-xF"#, "do-lowercase-version"),
    (r#"\C-xG"#, "do-lowercase-version"),
    (r#"\C-xH"#, "do-lowercase-version"),
    (r#"\C-xI"#, "do-lowercase-version"),
    (r#"\C-xJ"#, "do-lowercase-version"),
    (r#"\C-xK"#, "do-lowercase-version"),
    (r#"\C-xL"#, "do-lowercase-version"),
    (r#"\C-xM"#, "do-lowercase-version"),
    (r#"\C-xN"#, "do-lowercase-version"),
    (r#"\C-xO"#, "do-lowercase-version"),
    (r#"\C-xP"#, "do-lowercase-version"),
    (r#"\C-xQ"#, "do-lowercase-version"),
    (r#"\C-xR"#, "do-lowercase-version"),
    (r#"\C-xS"#, "do-lowercase-version"),
    (r#"\C-xT"#, "do-lowercase-version"),
    (r#"\C-xU"#, "do-lowercase-version"),
    (r#"\C-xV"#, "do-lowercase-version"),
    (r#"\C-xW"#, "do-lowercase-version"),
    (r#"\C-xX"#, "do-lowercase-version"),
    (r#"\C-xY"#, "do-lowercase-version"),
    (r#"\C-xZ"#, "do-lowercase-version"),
    (r#"\eA"#, "do-lowercase-version"),
    (r#"\eB"#, "do-lowercase-version"),
    (r#"\eC"#, "do-lowercase-version"),
    (r#"\eD"#, "do-lowercase-version"),
    (r#"\eE"#, "do-lowercase-version"),
    (r#"\eF"#, "do-lowercase-version"),
    (r#"\eG"#, "do-lowercase-version"),
    (r#"\eH"#, "do-lowercase-version"),
    (r#"\eI"#, "do-lowercase-version"),
    (r#"\eJ"#, "do-lowercase-version"),
    (r#"\eK"#, "do-lowercase-version"),
    (r#"\eL"#, "do-lowercase-version"),
    (r#"\eM"#, "do-lowercase-version"),
    (r#"\eN"#, "do-lowercase-version"),
    (r#"\eP"#, "do-lowercase-version"),
    (r#"\eQ"#, "do-lowercase-version"),
    (r#"\eR"#, "do-lowercase-version"),
    (r#"\eS"#, "do-lowercase-version"),
    (r#"\eT"#, "do-lowercase-version"),
    (r#"\eU"#, "do-lowercase-version"),
    (r#"\eV"#, "do-lowercase-version"),
    (r#"\eW"#, "do-lowercase-version"),
    (r#"\eX"#, "do-lowercase-version"),
    (r#"\eY"#, "do-lowercase-version"),
    (r#"\eZ"#, "do-lowercase-version"),
    (r#"\el"#, "downcase-word"),
    (r#"\e\C-i"#, "dynamic-complete-history"),
    (r#"\C-x\C-e"#, "edit-and-execute-command"),
    (r#"\C-x)"#, "end-kbd-macro"),
    (r#"\e>"#, "end-of-history"),
    (r#"\C-e"#, "end-of-line"),
    (r#"\eOF"#, "end-of-line"),
    (r#"\e[F"#, "end-of-line"),
    (r#"\C-x\C-x"#, "exchange-point-and-mark"),
    (r#"\ex"#, "execute-named-command"),
    (r#"\C-f"#, "forward-char"),
    (r#"\eOC"#, "forward-char"),
    (r#"\e[C"#, "forward-char"),
    (r#"\C-s"#, "forward-search-history"),
    (r#"\e[1;3C"#, "forward-word"),
    (r#"\e[1;5C"#, "forward-word"),
    (r#"\ef"#, "forward-word"),
    (r#"\eg"#, "glob-complete-word"),
    (r#"\C-x*"#, "glob-expand-word"),
    (r#"\C-xg"#, "glob-list-expansions"),
    (r#"\e^"#, "history-expand-line"),
    (r#"\e[5~"#, "history-search-backward"),
    (r#"\e[6~"#, "history-search-forward"),
    (r#"\e#"#, "insert-comment"),
    (r#"\e*"#, "insert-completions"),
    (r#"\e."#, "insert-last-argument"),
    (r#"\e_"#, "insert-last-argument"),
    (r#"\C-k"#, "kill-line"),
    (r#"\e[3;5~"#, "kill-word"),
    (r#"\ed"#, "kill-word"),
    (r#"\C-n"#, "next-history"),
    (r#"\eOB"#, "next-history"),
    (r#"\e[B"#, "next-history"),
    (r#"\en"#, "non-incremental-forward-search-history"),
    (r#"\ep"#, "non-incremental-reverse-search-history"),
    (r#"\C-o"#, "operate-and-get-next"),
    (r#"\e[2~"#, "overwrite-mode"),
    (r#"\C-x!"#, "possible-command-completions"),
    (r#"\e="#, "possible-completions"),
    (r#"\e?"#, "possible-completions"),
    (r#"\C-x/"#, "possible-filename-completions"),
    (r#"\C-x@"#, "possible-hostname-completions"),
    (r#"\C-x~"#, "possible-username-completions"),
    (r#"\C-x$"#, "possible-variable-completions"),
    (r#"\C-p"#, "previous-history"),
    (r#"\eOA"#, "previous-history"),
    (r#"\e[A"#, "previous-history"),
    (r#"\C-q"#, "quoted-insert"),
    (r#"\C-v"#, "quoted-insert"),
    (r#"\C-x\C-r"#, "re-read-init-file"),
    (r#"\C-r"#, "reverse-search-history"),
    (r#"\e\C-r"#, "revert-line"),
    (r#"\er"#, "revert-line"),
    (r#"\C-@"#, "set-mark"),
    (r#"\e "#, "set-mark"),
    (r#"\e\C-b"#, "shell-backward-word"),
    (r#"\e\C-e"#, "shell-expand-line"),
    (r#"\e\C-f"#, "shell-forward-word"),
    (r#"\e\C-d"#, "shell-kill-word"),
    (r#"\e\C-t"#, "shell-transpose-words"),
    (r#"\C-xs"#, "spell-correct-word"),
    (r#"\C-x("#, "start-kbd-macro"),
    (r#"\e&"#, "tilde-expand"),
    (r#"\C-t"#, "transpose-chars"),
    (r#"\et"#, "transpose-words"),
    (r#"\C-x\C-u"#, "undo"),
    (r#"\C-_"#, "undo"),
    (r#"\C-u"#, "unix-line-discard"),
    (r#"\C-w"#, "unix-word-rubout"),
    (r#"\eu"#, "upcase-word"),
    (r#"\C-y"#, "yank"),
    (r#"\e."#, "yank-last-arg"),
    (r#"\e_"#, "yank-last-arg"),
    (r#"\e\C-y"#, "yank-nth-arg"),
    (r#"\ey"#, "yank-pop"),
];

/// The vi-insert keymap bash starts in after `set -o vi`, from the same
/// pristine bash as [`EMACS_DEFAULTS`].
const VI_INSERT_DEFAULTS: &[(&str, &str)] = &[
    (r#"\C-j"#, "accept-line"),
    (r#"\C-m"#, "accept-line"),
    (r#"\eOD"#, "backward-char"),
    (r#"\e[D"#, "backward-char"),
    (r#"\C-h"#, "backward-delete-char"),
    (r#"\C-?"#, "backward-delete-char"),
    (r#"\e[1;3D"#, "backward-word"),
    (r#"\e[1;5D"#, "backward-word"),
    (r#"\eOH"#, "beginning-of-line"),
    (r#"\e[H"#, "beginning-of-line"),
    (r#"\e[200~"#, "bracketed-paste-begin"),
    (r#"\C-i"#, "complete"),
    (r#"\e[3~"#, "delete-char"),
    (r#"\eOF"#, "end-of-line"),
    (r#"\e[F"#, "end-of-line"),
    (r#"\eOC"#, "forward-char"),
    (r#"\e[C"#, "forward-char"),
    (r#"\C-s"#, "forward-search-history"),
    (r#"\e[1;3C"#, "forward-word"),
    (r#"\e[1;5C"#, "forward-word"),
    (r#"\e[5~"#, "history-search-backward"),
    (r#"\e[6~"#, "history-search-forward"),
    (r#"\e[3;5~"#, "kill-word"),
    (r#"\C-n"#, "menu-complete"),
    (r#"\C-p"#, "menu-complete-backward"),
    (r#"\eOB"#, "next-history"),
    (r#"\e[B"#, "next-history"),
    (r#"\e[2~"#, "overwrite-mode"),
    (r#"\eOA"#, "previous-history"),
    (r#"\e[A"#, "previous-history"),
    (r#"\C-v"#, "quoted-insert"),
    (r#"\C-r"#, "reverse-search-history"),
    (r#"\C-t"#, "transpose-chars"),
    (r#"\C-u"#, "unix-line-discard"),
    (r#"\C-d"#, "vi-eof-maybe"),
    (r#"\e"#, "vi-movement-mode"),
    (r#"\C-_"#, "vi-undo"),
    (r#"\C-w"#, "vi-unix-word-rubout"),
    (r#"\C-y"#, "yank"),
];

pub struct Readline;

impl Readline {
    pub fn inspect_input(&self, input: &TerminalInput) -> LayerResult {
        self.inspect_sequence(std::slice::from_ref(input))
    }

    /// Evaluate consecutive key presses as Readline would read them: a
    /// complete binding is dispatched and resets the keymap, a prefix key
    /// stays pending, and an unbound continuation aborts the pending prefix.
    /// The result describes what the last input does, e.g. `[\x18], [\x05]`
    /// resolves to `edit-and-execute-command`.
    pub fn inspect_sequence(&self, inputs: &[TerminalInput]) -> LayerResult {
        evaluate(&Keymap::load(), inputs)
    }
}

fn evaluate(keymap: &Keymap, inputs: &[TerminalInput]) -> LayerResult {
    let Some((last, earlier)) = inputs.split_last() else {
        return LayerResult::new(
            LAYER,
            LayerId::Shell,
            Outcome::Unknown,
            "no terminal input to evaluate",
            vec![],
        );
    };
    let mut pending = Vec::new();
    for input in earlier {
        pending.extend_from_slice(&input.bytes);
        match keymap.resolve(&pending) {
            Resolution::Prefix { .. } | Resolution::Bound { prefix_of: 1.., .. } => {}
            Resolution::Bound { .. } | Resolution::Unbound => pending.clear(),
        }
    }
    render(keymap, &pending, last)
}

fn render(keymap: &Keymap, pending: &[u8], last: &TerminalInput) -> LayerResult {
    let mut bytes = pending.to_vec();
    bytes.extend_from_slice(&last.bytes);
    let mut details = vec![
        format!("sequence: {}", format_bytes(&bytes)),
        format_input(last),
    ];
    if !pending.is_empty() {
        details.push(format!(
            "pending prefix from earlier keys: {}",
            readline_notation(pending)
        ));
    }
    // The action an unverified (non-snapshot) answer rests on, with the entry
    // whose source it came from when one is known.
    let mut default_action: Option<(String, Option<&Entry>)> = None;
    let resolution = keymap.resolve(&bytes);
    let tier = keymap.tier(&resolution);
    let (outcome, summary) = match resolution {
        Resolution::Bound {
            entry,
            continuations,
            prefix_of,
        } => {
            default_action = Some((entry.label(), Some(entry)));
            details.push(format!("binding source: {}", entry.source));
            if prefix_of > 0 {
                details.push(format!(
                    "{} is also a prefix; Readline waits keyseq-timeout for {}",
                    entry.notation,
                    format_continuations(&continuations, prefix_of)
                ));
            }
            (Outcome::Consumed, entry.summary())
        }
        Resolution::Prefix {
            continuations,
            prefix_of,
        } => {
            default_action = Some((
                "a prefix key (Readline waits for the next key)".into(),
                continuations.first().copied(),
            ));
            details.push(format!(
                "continues with {}",
                format_continuations(&continuations, prefix_of)
            ));
            details.push(format!("binding source: {}", keymap.source_label()));
            (
                Outcome::Consumed,
                format!(
                    "prefix key {}; Readline waits for the next key",
                    readline_notation(&bytes)
                ),
            )
        }
        Resolution::Unbound if !pending.is_empty() => {
            default_action = Some((
                format!(
                    "a bell after prefix {} (the sequence is discarded)",
                    readline_notation(pending)
                ),
                None,
            ));
            details.push(format!("binding source: {}", keymap.source_label()));
            (
                Outcome::Consumed,
                format!(
                    "{} is not bound after prefix {}; Readline rings the bell and discards the sequence",
                    readline_notation(&bytes),
                    readline_notation(pending)
                ),
            )
        }
        Resolution::Unbound if keymap.truncated => {
            details.push(
                "bind -P lists at most five keys per function; longer lists were truncated".into(),
            );
            details.push(
                "re-run `eval \"$(whykey shell-init bash)\"` to capture untruncated bind -p output"
                    .into(),
            );
            (
                Outcome::UncertainContinues,
                "not found in the truncated Bash bind -P snapshot; the key may still be bound"
                    .into(),
            )
        }
        Resolution::Unbound => {
            let lookup = match keymap.origin {
                Origin::Snapshot => "not bound in the current Readline keymap",
                Origin::Files { emacs: true } => "no Readline default or INPUTRC binding",
                Origin::Files { emacs: false } => {
                    details.push("vi mode: the vi-insert keymap bash starts in is assumed".into());
                    "no Readline vi-insert default or INPUTRC binding"
                }
            };
            (
                Outcome::Pass,
                format!("{lookup}: {}", unbound_effect(keymap, &bytes)),
            )
        }
    };
    append_mode_detail(&mut details);
    if let Some(alias) = terminal::legacy_byte_alias(&last.bytes) {
        details.push(alias.into());
    }
    if keymap.origin == Origin::Snapshot {
        return LayerResult::new(LAYER, LayerId::Shell, outcome, summary, details)
            .with_optional_tier(tier);
    }
    // Without the live `bind` snapshot, startup files can rebind any key
    // (fzf binds \C-r and \C-t with `bind -x`), so no answer is definitive.
    details.push(SHELL_INIT_HINT.into());
    let outcome = match outcome {
        Outcome::Consumed => Outcome::HandledUncertain,
        _ => Outcome::UncertainContinues,
    };
    let result =
        LayerResult::new(LAYER, LayerId::Shell, outcome, summary, details).with_optional_tier(tier);
    match default_action {
        Some((action, entry)) => result.with_binding(unverified_evidence(action, entry)),
        None => result,
    }
}

/// What Readline does with an unbound sequence. It is the last consumer, so
/// the key never goes further: Readline walks the keymap until the first
/// byte that neither completes nor continues a binding, rings the bell and
/// discards the bytes read so far, then reads the remaining bytes as new
/// input (so Shift+Up, `\e[1;2A`, inserts `A`).
fn unbound_effect(keymap: &Keymap, bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if text.chars().count() == 1 && !text.chars().any(char::is_control) {
            return format!("the printable key self-inserts {text:?}");
        }
    }
    let (stop, resolution) = (1..=bytes.len())
        .map(|end| (end, keymap.resolve(&bytes[..end])))
        .find(|(_, resolution)| !matches!(resolution, Resolution::Prefix { .. }))
        .unwrap_or((bytes.len(), Resolution::Unbound));
    let (discarded, rest) = bytes.split_at(stop);
    let mut effect = match resolution {
        Resolution::Bound { entry, .. } => format!(
            "Readline runs {} for {}",
            entry.label(),
            readline_notation(discarded)
        ),
        _ => format!(
            "Readline rings the bell and discards {}",
            readline_notation(discarded)
        ),
    };
    if !rest.is_empty() {
        if rest
            .iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
        {
            effect.push_str(&format!(
                ", then inserts the trailing {:?}",
                String::from_utf8_lossy(rest)
            ));
        } else {
            effect.push_str(&format!(
                ", then reads the trailing {} as new input",
                readline_notation(rest)
            ));
        }
    }
    effect
}

/// Evidence for a binding read from Readline defaults or INPUTRC rather than
/// the live shell, which may have rebound the key. `entry` supplies the
/// source: an INPUTRC file, or none for a built-in default.
fn unverified_evidence(action: String, entry: Option<&Entry>) -> BindingEvidence {
    let source = entry
        .and_then(|entry| {
            entry
                .source
                .strip_prefix("INPUTRC ")
                .or_else(|| entry.source.strip_prefix("bind in "))
        })
        .map(|path| SourceLocation::new(path, None));
    BindingEvidence::action(action, BindingScope::Unknown, source)
        .with_uncertainty(UncertaintyReason::UnverifiedDefault)
}

fn format_continuations(continuations: &[&Entry], total: usize) -> String {
    let mut listed = continuations
        .iter()
        .map(|entry| format!("{} ({})", entry.notation, entry.label()))
        .collect::<Vec<_>>()
        .join(", ");
    if total > continuations.len() {
        listed.push_str(&format!(", and {} more", total - continuations.len()));
    }
    listed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `whykey shell-init bash` captured the live `bind -P/-S/-X` output.
    Snapshot,
    /// INPUTRC files, startup-file `bind` commands and, in emacs mode, the
    /// built-in defaults.
    Files { emacs: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Function,
    Macro,
    ShellCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// Readline key notation as written by the source, e.g. `\C-x\C-e`.
    notation: String,
    bytes: Vec<u8>,
    action: Action,
    value: String,
    source: String,
    /// A Readline built-in default rather than a binding some file or the
    /// live shell declared.
    builtin: bool,
}

impl Entry {
    fn new(notation: &str, action: Action, value: &str, source: impl Into<String>) -> Self {
        Self {
            notation: notation.to_owned(),
            bytes: decode_sequence(notation),
            action,
            value: value.to_owned(),
            source: source.into(),
            builtin: false,
        }
    }

    fn label(&self) -> String {
        match self.action {
            Action::Function => self.value.clone(),
            Action::Macro if self.value.is_empty() => "macro".into(),
            Action::Macro => format!("macro {}", self.value),
            Action::ShellCommand => format!("shell command (bind -x): {}", self.value),
        }
    }

    fn summary(&self) -> String {
        match self.action {
            Action::ShellCommand => self.label(),
            Action::Function | Action::Macro => format!("bound to {}", self.label()),
        }
    }
}

enum Resolution<'a> {
    Bound {
        entry: &'a Entry,
        continuations: Vec<&'a Entry>,
        prefix_of: usize,
    },
    Prefix {
        continuations: Vec<&'a Entry>,
        prefix_of: usize,
    },
    Unbound,
}

/// Bindings in lookup order: the first entry for a byte sequence wins.
struct Keymap {
    entries: Vec<Entry>,
    origin: Origin,
    truncated: bool,
}

impl Keymap {
    fn load() -> Self {
        let var = |name: &str| env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        match var("WHYKEY_READLINE_BINDINGS") {
            Some(bindings) => Self::from_snapshot(
                &bindings,
                var("WHYKEY_READLINE_MACROS").as_deref(),
                var("WHYKEY_READLINE_SHELL_BINDINGS").as_deref(),
                active_keymap().as_deref(),
            ),
            None => {
                let bashrc = Bashrc::read(!readline_mode_is_emacs());
                Self::from_files(
                    &bashrc.effective(bashrc.vi_mode()),
                    &inputrc_entries(),
                    !bashrc.vi_mode(),
                )
            }
        }
    }

    /// The live snapshot is authoritative: built-in defaults are never
    /// consulted, because `bind -p` already lists every effective binding.
    /// Older wrappers exported `bind -P`, which truncates long key lists.
    fn from_snapshot(
        bindings: &str,
        macros: Option<&str>,
        shell_commands: Option<&str>,
        keymap: Option<&str>,
    ) -> Self {
        let mut entries = Vec::new();
        if let Some(snapshot) = shell_commands {
            entries.extend(quoted_pair_entries(
                snapshot,
                Action::ShellCommand,
                "current Bash bind -X snapshot",
            ));
        }
        if let Some(snapshot) = macros {
            entries.extend(quoted_pair_entries(
                snapshot,
                Action::Macro,
                "current Bash bind -S snapshot",
            ));
        }
        let mut truncated = false;
        for section in snapshot_sections(bindings, keymap) {
            for line in section.lines() {
                if let Some((notation, rest)) = leading_quoted(line.trim()) {
                    // `bind -p`: `"\C-x\C-e": edit-and-execute-command`
                    let Some(function) = rest.trim_start().strip_prefix(':') else {
                        continue;
                    };
                    entries.push(Entry::new(
                        notation,
                        Action::Function,
                        function.trim(),
                        "current Bash bind -p snapshot",
                    ));
                    continue;
                }
                let Some((function, keys)) = line.split_once(" can be found on ") else {
                    continue;
                };
                truncated |= keys.trim_end().ends_with("...");
                entries.extend(quoted_tokens(keys).into_iter().map(|notation| {
                    Entry::new(
                        notation,
                        Action::Function,
                        function.trim(),
                        "current Bash bind -P snapshot",
                    )
                }));
            }
        }
        Self {
            entries,
            origin: Origin::Snapshot,
            truncated,
        }
    }

    /// `bashrc` holds startup-file `bind` commands, last first; they run
    /// after INPUTRC is read, so they take precedence over it.
    fn from_files(bashrc: &[Entry], inputrc: &[Entry], emacs: bool) -> Self {
        let mut entries = bashrc.to_vec();
        // Later inputrc declarations override earlier ones.
        entries.extend(inputrc.iter().rev().cloned());
        let (defaults, source) = if emacs {
            (EMACS_DEFAULTS, "Readline default")
        } else {
            (VI_INSERT_DEFAULTS, "Readline vi-insert default")
        };
        entries.extend(defaults.iter().map(|(notation, function)| Entry {
            builtin: true,
            ..Entry::new(notation, Action::Function, function, source)
        }));
        Self {
            entries,
            origin: Origin::Files { emacs },
            truncated: false,
        }
    }

    fn source_label(&self) -> &'static str {
        match self.origin {
            Origin::Snapshot => "current Bash bind snapshot",
            Origin::Files { emacs: true } => {
                "Readline defaults, INPUTRC and startup-file bind commands"
            }
            Origin::Files { emacs: false } => {
                "Readline vi-insert defaults, INPUTRC and startup-file bind commands"
            }
        }
    }

    /// Where the answer for `resolution` came from: the live snapshot, the
    /// entries it rests on (built-in defaults or the user's INPUTRC and
    /// startup-file bindings), or, for an unbound key, every binding that
    /// was consulted.
    fn tier(&self, resolution: &Resolution<'_>) -> Option<EvidenceTier> {
        if self.origin == Origin::Snapshot {
            return Some(EvidenceTier::LiveQuery);
        }
        let from = |builtin: bool| {
            Some(if builtin {
                EvidenceTier::Default
            } else {
                EvidenceTier::ConfigFile
            })
        };
        match resolution {
            Resolution::Bound { entry, .. } => from(entry.builtin),
            Resolution::Prefix { continuations, .. } => {
                from(continuations.iter().all(|entry| entry.builtin))
            }
            Resolution::Unbound => from(self.entries.iter().all(|entry| entry.builtin)),
        }
    }

    fn resolve(&self, bytes: &[u8]) -> Resolution<'_> {
        let exact = self.entries.iter().find(|entry| entry.bytes == bytes);
        let mut seen: Vec<&[u8]> = Vec::new();
        let mut continuations = Vec::new();
        for entry in &self.entries {
            if entry.bytes.len() > bytes.len()
                && entry.bytes.starts_with(bytes)
                && !seen.contains(&entry.bytes.as_slice())
            {
                seen.push(&entry.bytes);
                if continuations.len() < MAX_CONTINUATIONS {
                    continuations.push(entry);
                }
            }
        }
        let prefix_of = seen.len();
        match exact {
            Some(entry) => Resolution::Bound {
                entry,
                continuations,
                prefix_of,
            },
            None if prefix_of > 0 => Resolution::Prefix {
                continuations,
                prefix_of,
            },
            None => Resolution::Unbound,
        }
    }
}

/// `bind -P` output sections for the active keymap, falling back to the
/// unlabelled current-keymap dump that precedes the first section marker.
fn snapshot_sections<'a>(snapshot: &'a str, keymap: Option<&str>) -> Vec<&'a str> {
    let current = snapshot
        .split("# WHYKEY_KEYMAP ")
        .next()
        .unwrap_or(snapshot);
    let section = keymap.and_then(|map| {
        let (_, rest) = snapshot.split_once(&format!("# WHYKEY_KEYMAP {map}\n"))?;
        Some(rest.split("# WHYKEY_KEYMAP ").next().unwrap_or(rest))
    });
    section.into_iter().chain([current]).collect()
}

/// Lines of `bind -X` (`"\C-r" "cmd"`) or `bind -S` (`\ec outputs text`).
fn quoted_pair_entries(snapshot: &str, action: Action, source: &'static str) -> Vec<Entry> {
    snapshot
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (notation, value) = match line.split_once(" outputs ") {
                Some((notation, value)) => (notation.trim().trim_matches('"'), value.trim()),
                None => split_quoted_tokens(line)?,
            };
            Some(Entry::new(notation, action, value, source))
        })
        .collect()
}

/// A line that starts with a quoted token: its contents and the text after
/// the closing quote. `None` when the line does not open with a quote.
fn leading_quoted(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix('"')?;
    let token = *quoted_tokens(line).first()?;
    // The token starts at byte 1, so its closing quote is at 1 + len.
    Some((token, &line[token.len() + 2..]))
}

/// Contents of every double-quoted token, honouring backslash escapes.
fn quoted_tokens(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        match (start, character) {
            (None, '"') => start = Some(index + 1),
            (None, _) => {}
            (Some(_), _) if escaped => escaped = false,
            (Some(_), '\\') => escaped = true,
            (Some(begin), '"') => {
                tokens.push(&text[begin..index]);
                start = None;
            }
            (Some(_), _) => {}
        }
    }
    tokens
}

fn format_input(input: &TerminalInput) -> String {
    let bytes = input.display_bytes();
    format!(
        "normal input: {bytes} ({}, {})",
        input.confidence_label(),
        input.description
    )
}

/// Readline key notation for raw bytes, e.g. `[0x18, 0x05]` → `\C-x\C-e`.
fn readline_notation(bytes: &[u8]) -> String {
    let mut notation = String::new();
    for &byte in bytes {
        match byte {
            0x1b => notation.push_str(r"\e"),
            0x7f => notation.push_str(r"\C-?"),
            0x00..=0x1f => {
                notation.push_str(r"\C-");
                notation.push(char::from(byte + b'@').to_ascii_lowercase());
            }
            b'\\' => notation.push_str(r"\\"),
            b'"' => notation.push_str("\\\""),
            0x20..=0x7e => notation.push(char::from(byte)),
            _ => notation.push_str(&format!(r"\x{byte:02x}")),
        }
    }
    notation
}

fn readline_mode_is_emacs() -> bool {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_VARIABLES") {
        let snapshot = snapshot.to_string_lossy().to_ascii_lowercase();
        return !snapshot.contains("editing-mode is set to `vi'")
            && !snapshot.contains("editing-mode is set to 'vi'")
            && !snapshot.contains("keymap is set to `vi")
            && !snapshot.contains("keymap is set to 'vi");
    }

    for path in inputrc_paths() {
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(is_emacs) = inputrc_editing_mode_is_emacs(&content) {
            return is_emacs;
        }
    }
    true
}

fn inputrc_editing_mode_is_emacs(content: &str) -> Option<bool> {
    content.lines().rev().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("set editing-mode")?.trim();
        match value.to_ascii_lowercase().as_str() {
            "emacs" => Some(true),
            "vi" => Some(false),
            _ => None,
        }
    })
}

fn active_keymap() -> Option<String> {
    let snapshot = env::var_os("WHYKEY_READLINE_VARIABLES")?
        .to_string_lossy()
        .into_owned();
    snapshot.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        let value = lower.strip_prefix("keymap is set to ")?;
        Some(value.trim().trim_matches(['`', '\'', '"']).to_owned())
    })
}

fn append_mode_detail(details: &mut Vec<String>) {
    if let Some(snapshot) = env::var_os("WHYKEY_READLINE_VARIABLES") {
        let snapshot = snapshot.to_string_lossy();
        if let Some(line) = snapshot.lines().find(|line| {
            line.to_ascii_lowercase()
                .starts_with("editing-mode is set to")
        }) {
            details.push(line.trim().to_owned());
        }
        if let Some(line) = snapshot
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("keymap is set to"))
        {
            details.push(line.trim().to_owned());
        }
    }
}

/// Every quoted key binding declared by the effective inputrc, in file
/// order (includes expanded in place).
fn inputrc_entries() -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(path) = inputrc_paths().into_iter().find(|path| path.is_file()) {
        inputrc_entries_in_path(&path, 0, &mut entries);
    }
    entries
}

fn inputrc_entries_in_path(path: &Path, depth: usize, entries: &mut Vec<Entry>) {
    const MAX_INCLUDE_DEPTH: usize = 16;
    if depth > MAX_INCLUDE_DEPTH {
        return;
    }
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let source = format!("INPUTRC {}", path.display());
    let mut active = true;
    let mut conditions = Vec::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(include) = line.strip_prefix("$include") {
            if active {
                let include = resolve_inputrc_include(include.trim(), base);
                inputrc_entries_in_path(&include, depth + 1, entries);
            }
            continue;
        }
        if let Some(condition) = line.strip_prefix("$if") {
            let matches = inputrc_condition(condition.trim());
            conditions.push((active, matches));
            active = active && matches;
            continue;
        }
        if line == "$else" {
            if let Some((parent, matches)) = conditions.last().copied() {
                active = parent && !matches;
            }
            continue;
        }
        if line == "$endif" {
            if let Some((parent, _)) = conditions.pop() {
                active = parent;
            }
            continue;
        }
        if !active {
            continue;
        }
        if let Some(entry) = binding_line(line, false, &source) {
            entries.push(entry);
        }
    }
}

/// One Readline binding line, `"\C-t": transpose-chars` or
/// `"\C-t": "macro text"`; with `bind -x` the value is a shell command.
fn binding_line(line: &str, shell_command: bool, source: &str) -> Option<Entry> {
    let (notation, rest) = leading_quoted(line)?;
    let value = rest.trim_start().strip_prefix(':')?.trim();
    let quoted = value
        .strip_prefix('"')
        .map(|text| text.strip_suffix('"').unwrap_or(text));
    Some(match (shell_command, quoted) {
        (true, text) => Entry::new(
            notation,
            Action::ShellCommand,
            text.unwrap_or(value),
            source,
        ),
        (false, Some(text)) => Entry::new(notation, Action::Macro, text, source),
        (false, None) => Entry::new(notation, Action::Function, value, source),
    })
}

/// `bind` commands and `set -o vi|emacs` read statically from the startup
/// files an interactive Bash runs, in execution order.
#[derive(Debug, Default)]
struct Bashrc {
    /// Bindings with the prompt keymap they landed in (see `prompt_keymap`).
    entries: Vec<(Entry, Option<bool>)>,
    /// The editing mode INPUTRC selects, before any startup file runs.
    inputrc_vi: bool,
    /// The editing mode the files leave, when they set one.
    vi: Option<bool>,
}

impl Bashrc {
    fn read(inputrc_vi: bool) -> Self {
        let mut bashrc = Self {
            inputrc_vi,
            ..Self::default()
        };
        for path in bashrc_paths() {
            if let Ok(text) = fs::read_to_string(&path) {
                bashrc.apply_script(&text, &path);
            }
        }
        bashrc
    }

    /// The editing mode every prompt starts in.
    fn vi_mode(&self) -> bool {
        self.vi.unwrap_or(self.inputrc_vi)
    }

    fn apply_script(&mut self, script: &str, path: &Path) {
        for words in crate::layers::shell::posix_commands(script) {
            match crate::layers::shell::command_words(&words) {
                [command, arguments @ ..] if command.text == "set" => {
                    for pair in arguments.windows(2) {
                        if pair[0].text == "-o" {
                            match pair[1].text.as_str() {
                                "vi" => self.vi = Some(true),
                                "emacs" => self.vi = Some(false),
                                _ => {}
                            }
                        }
                    }
                }
                [command, arguments @ ..] if command.text == "bind" => {
                    self.apply_bind(arguments, path);
                }
                _ => {}
            }
        }
    }

    fn apply_bind(&mut self, words: &[crate::layers::shell::Word], path: &Path) {
        let source = format!("bind in {}", path.display());
        let mut keymap: Option<String> = None;
        let mut shell_command = false;
        let mut index = 0;
        while let Some(word) = words.get(index) {
            match word.text.as_str() {
                "-m" => {
                    index += 1;
                    keymap = words.get(index).map(|word| word.text.clone());
                }
                "-x" => shell_command = true,
                "-f" => {
                    index += 1;
                    if let Some(file) = words.get(index).filter(|word| !word.expands) {
                        let mut entries = Vec::new();
                        inputrc_entries_in_path(Path::new(&file.text), 0, &mut entries);
                        let target = self.prompt_keymap(keymap.as_deref());
                        self.entries
                            .extend(entries.into_iter().map(|entry| (entry, target)));
                    }
                }
                // Queries and listings (-p, -P, -l, -q, -r, -s, -u, -v, ...)
                // bind nothing that can be resolved statically.
                text if text.starts_with('-') => return,
                _ => break,
            }
            index += 1;
        }
        let target = self.prompt_keymap(keymap.as_deref());
        for word in words[index.min(words.len())..]
            .iter()
            .filter(|word| !word.expands)
        {
            let line = word.text.trim();
            if let Some(mode) = line.strip_prefix("set editing-mode") {
                match mode.trim() {
                    "vi" => self.vi = Some(true),
                    "emacs" => self.vi = Some(false),
                    _ => {}
                }
                continue;
            }
            if let Some(entry) = binding_line(line, shell_command, &source) {
                self.entries.push((entry, target));
            }
        }
    }

    /// Whether a binding lands in the vi insertion keymap (`Some(true)`), the
    /// emacs keymap (`Some(false)`) or another keymap no prompt starts in.
    /// `bind` without `-m` binds in the keymap of the current editing mode.
    fn prompt_keymap(&self, keymap: Option<&str>) -> Option<bool> {
        match keymap {
            None => Some(self.vi.unwrap_or(self.inputrc_vi)),
            Some("emacs" | "emacs-standard") => Some(false),
            Some("vi-insert") => Some(true),
            Some(_) => None,
        }
    }

    /// Bindings of the keymap a prompt starts in, last definition first.
    fn effective(&self, vi: bool) -> Vec<Entry> {
        self.entries
            .iter()
            .rev()
            .filter(|(_, target)| *target == Some(vi))
            .map(|(entry, _)| entry.clone())
            .collect()
    }
}

/// Startup files of an interactive non-login Bash: the system bashrc
/// (Debian builds read /etc/bash.bashrc) and ~/.bashrc.
fn bashrc_paths() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/etc/bash.bashrc")];
    if let Some(home) = env::var_os("HOME").filter(|home| !home.is_empty()) {
        paths.push(PathBuf::from(home).join(".bashrc"));
    }
    paths
}

fn resolve_inputrc_include(raw: &str, base: &Path) -> PathBuf {
    let raw = raw.trim().trim_matches(['"', '\'']);
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        if raw == "~" {
            return home;
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn inputrc_condition(condition: &str) -> bool {
    let condition = condition.trim().to_ascii_lowercase();
    if condition == "bash" {
        // whykey only reaches this parser after identifying Bash as the
        // current shell, so the application-name conditional is satisfied.
        return true;
    }
    if let Some(term) = condition.strip_prefix("term=") {
        let current = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
        return if let Some(prefix) = term.strip_suffix('*') {
            current.starts_with(prefix)
        } else {
            current == term
        };
    }
    match condition.strip_prefix("mode=") {
        Some("emacs") => readline_mode_is_emacs(),
        Some("vi") | Some("vi-insertion") | Some("vi-command") => !readline_mode_is_emacs(),
        Some(_) => false,
        None => false,
    }
}

fn inputrc_paths() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("INPUTRC") {
        return vec![PathBuf::from(path)];
    }
    if let Some(home) = env::var_os("HOME") {
        let user = PathBuf::from(home).join(".inputrc");
        if user.is_file() {
            return vec![user];
        }
    }
    vec![PathBuf::from("/etc/inputrc")]
}

/// Split `"seq" "value"` (bind -X) or `"seq": "value"` (bind -s) lines.
fn split_quoted_tokens(line: &str) -> Option<(&str, &str)> {
    let (notation, rest) = leading_quoted(line.trim())?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim();
    let value = rest.strip_prefix('"')?.strip_suffix('"')?;
    Some((notation, value))
}

/// Decode Readline key notation (`\C-x`, `\e`, `\M-x`, `\C-?`, `\\`,
/// `\"`, octal and hex escapes) into the bytes the terminal sends.
fn decode_sequence(sequence: &str) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(sequence.len());
    let mut chars = sequence.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\\' {
            let mut buffer = [0; 4];
            decoded.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        let Some(escaped) = chars.next() else {
            decoded.push(b'\\');
            break;
        };
        match escaped {
            'e' | 'E' => decoded.push(0x1b),
            'C' if chars.peek() == Some(&'-') => {
                chars.next();
                match chars.next() {
                    Some('?') => decoded.push(0x7f),
                    Some('\\') => {
                        // `\C-\\` is Ctrl+backslash; `\C-\]` style escapes
                        // quote the following character.
                        let quoted = chars.next().unwrap_or('\\');
                        decoded.push(quoted as u8 & 0x1f);
                    }
                    Some(control) => decoded.push(control.to_ascii_uppercase() as u8 & 0x1f),
                    None => {}
                }
            }
            'M' if chars.peek() == Some(&'-') => {
                chars.next();
                decoded.push(0x1b);
            }
            'x' => {
                let mut hex = String::new();
                while hex.len() < 2 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
                    hex.extend(chars.next());
                }
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    decoded.push(byte);
                }
            }
            '0'..='7' => {
                let mut octal = String::from(escaped);
                while octal.len() < 3 && chars.peek().is_some_and(|c| ('0'..='7').contains(c)) {
                    octal.extend(chars.next());
                }
                if let Ok(byte) = u8::from_str_radix(&octal, 8) {
                    decoded.push(byte);
                }
            }
            'a' => decoded.push(0x07),
            'b' => decoded.push(0x08),
            'd' => decoded.push(0x7f),
            'f' => decoded.push(0x0c),
            'n' => decoded.push(b'\n'),
            'r' => decoded.push(b'\r'),
            't' => decoded.push(b'\t'),
            'v' => decoded.push(0x0b),
            other => {
                let mut buffer = [0; 4];
                decoded.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE_BIND_P: &str = r#""\C-g": abort
"\C-x\C-g": abort
"\e\C-g": abort
"\C-]": character-search
"\e\C-l": clear-display
"\C-l": clear-screen
"\e{": complete-into-braces
"\e\\": delete-horizontal-space
"\e-": digit-argument
"\e0": digit-argument
"\e7": digit-argument
"\C-x\C-v": display-shell-version
"\C-x\C-e": edit-and-execute-command
"\C-x\C-x": exchange-point-and-mark
"\C-x\C-r": re-read-init-file
"\C-_": undo
"\C-x\C-u": undo
# alias-expand-line (not bound)
# WHYKEY_KEYMAP emacs
"\C-l": clear-screen
# WHYKEY_KEYMAP vi-insertion
"\e": vi-movement-mode"#;
    const LIVE_BIND_X: &str = r#""\C-r" "__fzf_history__"
"\C-t" "fzf-file-widget""#;
    const LIVE_BIND_S: &str = r"\ec outputs  \C-b\C-k \C-u`__fzf_cd__`";

    fn live() -> Keymap {
        Keymap::from_snapshot(
            LIVE_BIND_P,
            Some(LIVE_BIND_S),
            Some(LIVE_BIND_X),
            Some("emacs"),
        )
    }

    fn defaults() -> Keymap {
        Keymap::from_files(&[], &[], true)
    }

    fn key(bytes: &[u8]) -> TerminalInput {
        TerminalInput::predicted(bytes.to_vec(), "legacy control byte")
    }

    fn inspect(keymap: &Keymap, keys: &[&[u8]]) -> LayerResult {
        let inputs: Vec<_> = keys.iter().map(|bytes| key(bytes)).collect();
        evaluate(keymap, &inputs)
    }

    #[test]
    fn default_tables_match_pristine_bash_5_3() {
        // `bind -p | grep -vc self-insert` in the pristine lab bash 5.3.9.
        assert_eq!(EMACS_DEFAULTS.len(), 176);
        assert_eq!(VI_INSERT_DEFAULTS.len(), 39);
        let keymap = defaults();
        for (bytes, action) in [
            (&b"\x1b1"[..], "digit-argument"),
            (b"\x1b-", "digit-argument"),
            (b"\x1b\\", "delete-horizontal-space"),
            (b"\x1bA", "do-lowercase-version"),
            (b"\x18A", "do-lowercase-version"),
        ] {
            let result = inspect(&keymap, &[bytes]);
            assert_eq!(result.summary, format!("bound to {action}"), "{bytes:?}");
        }
        let vi = Keymap::from_files(&[], &[], false);
        assert_eq!(inspect(&vi, &[b"\x0e"]).summary, "bound to menu-complete");
    }

    #[test]
    fn unbound_sequences_state_what_readline_does() {
        let snapshot = EMACS_DEFAULTS
            .iter()
            .map(|(notation, function)| format!("\"{notation}\": {function}"))
            .collect::<Vec<_>>()
            .join("\n");
        let keymap = Keymap::from_snapshot(&snapshot, None, None, Some("emacs"));
        for (bytes, effect) in [
            (
                &b"\x1b[1;2A"[..],
                r#"Readline rings the bell and discards \e[1;2, then inserts the trailing "A""#,
            ),
            (
                b"\x1b[15~",
                r#"Readline rings the bell and discards \e[15, then inserts the trailing "~""#,
            ),
            (
                b"\x1b[15;5~",
                r#"Readline rings the bell and discards \e[15, then inserts the trailing ";5~""#,
            ),
        ] {
            let result = inspect(&keymap, &[bytes]);
            assert_eq!(result.outcome, Outcome::Pass);
            assert_eq!(
                result.summary,
                format!("not bound in the current Readline keymap: {effect}")
            );
        }
    }

    #[test]
    fn startup_file_bind_commands_override_defaults() {
        let mut bashrc = Bashrc::default();
        bashrc.apply_script(
            r#"
bind -x '"\C-t": touch /tmp/fired'
bind '"\C-g": "macro text"'
if [[ $- == *i* ]]; then bind -m vi-command '"\C-a": vi-thing'; fi
bind "$var"
"#,
            Path::new("/home/user/.bashrc"),
        );
        let keymap = Keymap::from_files(&bashrc.effective(bashrc.vi_mode()), &[], true);
        let result = inspect(&keymap, &[&[0x14]]);
        assert_eq!(result.summary, "shell command (bind -x): touch /tmp/fired");
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(
            inspect(&keymap, &[&[0x07]]).summary,
            "bound to macro macro text"
        );
        assert_eq!(
            inspect(&keymap, &[&[0x01]]).summary,
            "bound to beginning-of-line"
        );

        let mut vi = Bashrc::default();
        vi.apply_script("set -o vi\nbind '\"\\C-t\": x'", Path::new("/r/.bashrc"));
        assert!(vi.vi_mode());
        assert_eq!(vi.effective(true).len(), 1);
        assert!(vi.effective(false).is_empty());
    }

    #[test]
    fn decodes_readline_key_notation() {
        assert_eq!(decode_sequence(r"\e[1;5D"), b"\x1b[1;5D");
        assert_eq!(decode_sequence(r"\C-f"), [0x06]);
        assert_eq!(decode_sequence(r"\C-x\C-e"), [0x18, 0x05]);
        assert_eq!(decode_sequence(r"\C-]"), [0x1d]);
        assert_eq!(decode_sequence(r"\C-?"), [0x7f]);
        assert_eq!(decode_sequence(r"\e\\"), [0x1b, b'\\']);
        assert_eq!(decode_sequence(r"\M-x"), [0x1b, b'x']);
        assert_eq!(decode_sequence(r"\x1b"), [0x1b]);
        assert_eq!(decode_sequence(r"\033"), [0x1b]);
    }

    #[test]
    fn default_table_matches_gnu_readline_emacs_bindings() {
        let keymap = defaults();
        let bound = |bytes: &[u8]| match keymap.resolve(bytes) {
            Resolution::Bound { entry, .. } => Some(entry.value.clone()),
            _ => None,
        };
        assert_eq!(bound(&[0x0c]).as_deref(), Some("clear-screen"));
        assert_eq!(bound(&[0x1d]).as_deref(), Some("character-search"));
        assert_eq!(bound(b"\x1b{").as_deref(), Some("complete-into-braces"));
        assert_eq!(bound(&[0x18]), None);
        assert!(matches!(keymap.resolve(&[0x18]), Resolution::Prefix { .. }));
    }

    #[test]
    fn ctrl_x_is_a_prefix_key_in_the_live_snapshot() {
        let result = inspect(&live(), &[&[0x18]]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result.summary.starts_with(r"prefix key \C-x"),
            "{}",
            result.summary
        );
        let continuations = result
            .details
            .iter()
            .find(|detail| detail.starts_with("continues with"))
            .unwrap();
        assert!(continuations.contains(r"\C-x\C-g (abort)"));
        assert!(continuations.contains("more"));
        assert!(!result.summary.contains("complete-into-braces"));
    }

    #[test]
    fn live_snapshot_is_authoritative_over_defaults() {
        // \C-o is a Readline default (operate-and-get-next) but absent from
        // this snapshot, so it must not be claimed.
        let result = inspect(&live(), &[&[0x0f]]);
        assert_eq!(result.outcome, Outcome::Pass);
        assert_eq!(
            result.summary,
            r"not bound in the current Readline keymap: Readline rings the bell and discards \C-o"
        );

        let result = inspect(&live(), &[&[0x0c]]);
        assert_eq!(result.summary, "bound to clear-screen");
        assert_eq!(result.outcome, Outcome::Consumed);
    }

    #[test]
    fn defaults_without_snapshot_are_uncertain_and_point_to_shell_init() {
        let result = inspect(&defaults(), &[&[0x12]]);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert_eq!(result.summary, "bound to reverse-search-history");
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("whykey shell-init bash"))
        );
        let evidence = result
            .binding
            .expect("the default binding is typed evidence");
        assert_eq!(evidence.action.as_deref(), Some("reverse-search-history"));
        assert_eq!(
            evidence.uncertainty,
            Some(UncertaintyReason::UnverifiedDefault)
        );
        assert!(evidence.source.is_none());

        let unbound = inspect(&defaults(), &[&[0x1c]]);
        assert_eq!(unbound.outcome, Outcome::UncertainContinues);
        assert!(unbound.binding.is_none());

        // The live snapshot is authoritative and carries no such caveat.
        assert!(inspect(&live(), &[&[0x0c]]).binding.is_none());
    }

    #[test]
    fn default_prefix_key_without_snapshot_is_an_unverified_default() {
        let result = inspect(&defaults(), &[&[0x18]]);
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("whykey shell-init bash"))
        );
        let evidence = result
            .binding
            .expect("a default prefix key is typed evidence");
        assert_eq!(
            evidence.uncertainty,
            Some(UncertaintyReason::UnverifiedDefault)
        );
        assert!(
            evidence
                .action
                .is_some_and(|action| action.contains("prefix"))
        );
        assert!(evidence.source.is_none());

        // The live snapshot is authoritative: its prefix carries no caveat.
        assert!(inspect(&live(), &[&[0x18]]).binding.is_none());
    }

    #[test]
    fn labels_bind_x_as_shell_command_and_keeps_macros() {
        let result = inspect(&live(), &[&[0x12]]);
        assert_eq!(result.summary, "shell command (bind -x): __fzf_history__");
        assert!(!result.summary.contains("macro"));
        assert!(
            result
                .details
                .contains(&"binding source: current Bash bind -X snapshot".to_owned())
        );

        let result = inspect(&live(), &[b"\x1bc"]);
        assert!(
            result.summary.starts_with(r"bound to macro \C-b\C-k"),
            "{}",
            result.summary
        );
    }

    #[test]
    fn resolves_multi_key_sequences_like_readline() {
        let result = inspect(&live(), &[&[0x18], &[0x05]]);
        assert_eq!(result.summary, "bound to edit-and-execute-command");
        assert_eq!(result.outcome, Outcome::Consumed);

        // A complete binding resets the keymap before the next key.
        let result = inspect(&live(), &[&[0x0c], &[0x1d]]);
        assert_eq!(result.summary, "bound to character-search");

        // An unbound continuation aborts the pending prefix.
        let result = inspect(&live(), &[&[0x18], b"q"]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(result.summary.contains(r"after prefix \C-x"));
    }

    #[test]
    fn keymap_section_and_quoted_backslash_keys_are_parsed() {
        let keymap = live();
        assert!(matches!(
            keymap.resolve(b"\x1b\\"),
            Resolution::Bound { entry, .. } if entry.value == "delete-horizontal-space"
        ));
        // vi-insertion's lone \e must not leak into the emacs keymap.
        assert!(matches!(keymap.resolve(&[0x1b]), Resolution::Prefix { .. }));
        assert!(!keymap.truncated);
    }

    #[test]
    fn states_escape_ambiguity_for_ctrl_bracketleft() {
        let result = inspect(&live(), &[&[0x1b]]);
        assert!(
            result
                .details
                .iter()
                .any(|detail| detail.contains("Escape"))
        );
    }

    #[test]
    fn parses_quoted_snapshot_tokens() {
        assert_eq!(
            split_quoted_tokens(r#""\e[1;5D" "backward-word""#),
            Some((r"\e[1;5D", "backward-word"))
        );
        assert_eq!(
            split_quoted_tokens(r#""\ec": "text""#),
            Some((r"\ec", "text"))
        );
        assert_eq!(
            quoted_tokens(r#""\C-x\"", "\e\\"."#),
            [r#"\C-x\""#, r"\e\\"]
        );
    }

    #[test]
    fn malformed_snapshot_lines_with_multibyte_prefix_are_skipped() {
        assert_eq!(split_quoted_tokens(r#"ééé "a" "b""#), None);
        assert_eq!(split_quoted_tokens("\""), None);
        let keymap = Keymap::from_snapshot(
            "ééé \"\\C-a\": beginning-of-line\n",
            None,
            Some("ééé \"a\" \"b\"\n"),
            None,
        );
        assert!(matches!(keymap.resolve(b"a"), Resolution::Unbound));
        assert!(matches!(keymap.resolve(&[0x01]), Resolution::Unbound));
    }

    #[test]
    fn parses_untruncated_bind_p_output() {
        let snapshot = "\"\\C-x\\C-e\": edit-and-execute-command\n# self-insert (not bound)\n\"\\e7\": digit-argument\n";
        let keymap = Keymap::from_snapshot(snapshot, None, None, None);
        let result = inspect(&keymap, &[b"\x1b7"]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert_eq!(result.summary, "bound to digit-argument");
        let result = inspect(&keymap, &[&[0x18]]);
        assert_eq!(result.outcome, Outcome::Consumed);
        assert!(
            result.summary.starts_with("prefix key"),
            "{}",
            result.summary
        );
    }

    #[test]
    fn truncated_bind_capital_p_snapshot_is_uncertain_when_unbound() {
        let snapshot = "digit-argument can be found on \"\\e-\", \"\\e0\", \"\\e1\", \"\\e2\", \"\\e3\", ...\n";
        let keymap = Keymap::from_snapshot(snapshot, None, None, None);
        let result = inspect(&keymap, &[b"\x1b7"]);
        assert_eq!(result.outcome, Outcome::UncertainContinues);
        assert!(result.summary.contains("truncated"), "{}", result.summary);
        let result = inspect(&keymap, &[b"\x1b0"]);
        assert_eq!(result.summary, "bound to digit-argument");
    }

    #[test]
    fn ignores_unknown_inputrc_conditions() {
        assert!(!inputrc_condition("term=unknown-terminal"));
    }

    #[test]
    fn recognizes_the_bash_application_condition() {
        assert!(inputrc_condition("Bash"));
    }

    #[test]
    fn reads_editing_mode_from_inputrc() {
        assert_eq!(
            inputrc_editing_mode_is_emacs("set editing-mode vi\n"),
            Some(false)
        );
        assert_eq!(
            inputrc_editing_mode_is_emacs("set editing-mode vi\nset editing-mode emacs\n"),
            Some(true)
        );
        assert_eq!(inputrc_editing_mode_is_emacs("set bell-style none\n"), None);
    }

    #[test]
    fn follows_literal_inputrc_includes_and_applies_later_overrides() {
        let base = std::env::temp_dir().join(format!("whykey-inputrc-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let included = base.join("system.inputrc");
        let root = base.join("user.inputrc");
        fs::write(&included, "\"\\C-f\": included\n").unwrap();
        fs::write(&root, "$include system.inputrc\n\"\\C-f\": local\n").unwrap();

        let mut entries = Vec::new();
        inputrc_entries_in_path(&root, 0, &mut entries);
        let keymap = Keymap::from_files(&[], &entries, true);
        let result = inspect(&keymap, &[&[0x06]]);
        assert_eq!(result.summary, "bound to local");
        assert_eq!(result.outcome, Outcome::HandledUncertain);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn tier_names_the_source_the_answer_rests_on() {
        let unbound: &[u8] = b"\x1b[99~";
        assert_eq!(
            inspect(&live(), &[&[0x14]]).tier,
            Some(EvidenceTier::LiveQuery)
        );
        assert_eq!(
            inspect(&live(), &[unbound]).tier,
            Some(EvidenceTier::LiveQuery)
        );
        assert_eq!(
            inspect(&defaults(), &[&[0x14]]).tier,
            Some(EvidenceTier::Default)
        );
        assert_eq!(
            inspect(&defaults(), &[unbound]).tier,
            Some(EvidenceTier::Default)
        );
        // vi mode with no user binding: the vi-insert defaults were read.
        let vi = Keymap::from_files(&[], &[], false);
        assert_eq!(inspect(&vi, &[unbound]).tier, Some(EvidenceTier::Default));

        let mut bashrc = Bashrc::default();
        bashrc.apply_script(
            "bind -x '\"\\C-t\": touch /tmp/fired'\n",
            Path::new("/home/user/.bashrc"),
        );
        let keymap = Keymap::from_files(&bashrc.effective(false), &[], true);
        assert_eq!(
            inspect(&keymap, &[&[0x14]]).tier,
            Some(EvidenceTier::ConfigFile)
        );
        // A built-in default still answers for its own key, while an
        // unbound key consulted the user's bindings too.
        assert_eq!(
            inspect(&keymap, &[&[0x01]]).tier,
            Some(EvidenceTier::Default)
        );
        assert_eq!(
            inspect(&keymap, &[unbound]).tier,
            Some(EvidenceTier::ConfigFile)
        );
    }
}
