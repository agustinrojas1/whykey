//! Compositor-independent XKB keymap compilation and inspection.
//!
//! The direct libxkbcommon API is intentionally not a hard build dependency:
//! many supported systems do not ship its development headers.  `xkbcli`
//! uses the same libxkbcommon compiler and gives us a bounded, read-only
//! fallback when RMLVO data is available from the environment.
//!
//! Modifier-state boundary (issue #74): Hyprland's key-event socket exposes
//! the current pressed modifier mask, while its keyboard snapshot exposes
//! CapsLock/NumLock LEDs. Neither source exposes latched Shift/AltGr (or
//! latched/locked modifier masks beyond those LEDs), so the capture path keeps
//! `latched` conditional and supplies only the observed lock state. Sway's
//! binding event and `get_bindings`/`get_tree` IPC likewise expose no latched
//! or lock-state fields. We therefore keep `XkbState` shift-only for latch
//! selection and never infer missing state from a US layout.

use std::collections::HashMap;
use std::env;
use std::process::Command;
use std::sync::LazyLock;

use crate::command;

type KeycodeMap = HashMap<String, Vec<u32>>;
type SymbolGroupMap = HashMap<String, HashMap<usize, Vec<String>>>;
/// Explicit `type=` assignments per key; the `None` group applies to every
/// group without its own `type[N]=`.
type KeyTypeMap = HashMap<String, HashMap<Option<usize>, String>>;

/// Modifiers an XKB key type can map, reduced to the state [`XkbState`]
/// observes. Virtual modifiers resolve to the real modifier they sit on in
/// the stock keymaps (LevelThree on Mod5, NumLock on Mod2).
const TYPE_SHIFT: u8 = 1;
const TYPE_LOCK: u8 = 1 << 1;
const TYPE_LEVEL_THREE: u8 = 1 << 2;
const TYPE_NUM_LOCK: u8 = 1 << 3;
/// Any other modifier (Control, Alt, Super, LevelFive, ...). `XkbState`
/// never observes one, so map entries naming it never match.
const TYPE_OTHER: u8 = 1 << 4;

/// The stock key types libxkbcommon assigns automatically, used when a
/// keymap omits its `xkb_types` section (fixtures, hand-written keymaps).
/// A compiled keymap's own definitions always take precedence.
const BUILTIN_TYPES: &str = r#"
xkb_types "builtin" {
    type "ONE_LEVEL" { modifiers= none; };
    type "TWO_LEVEL" { modifiers= Shift; map[Shift]= 2; };
    type "ALPHABETIC" { modifiers= Shift+Lock; map[Shift]= 2; map[Lock]= 2; };
    type "KEYPAD" { modifiers= NumLock; map[NumLock]= 2; };
    type "FOUR_LEVEL" {
        modifiers= Shift+LevelThree;
        map[Shift]= 2; map[LevelThree]= 3; map[Shift+LevelThree]= 4;
    };
    type "FOUR_LEVEL_ALPHABETIC" {
        modifiers= Shift+Lock+LevelThree;
        map[Shift]= 2; map[Lock]= 2; map[LevelThree]= 3; map[Shift+LevelThree]= 4;
        map[Lock+LevelThree]= 4; map[Shift+Lock+LevelThree]= 3;
    };
    type "FOUR_LEVEL_SEMIALPHABETIC" {
        modifiers= Shift+Lock+LevelThree;
        map[Shift]= 2; map[Lock]= 2; map[LevelThree]= 3; map[Shift+LevelThree]= 4;
        map[Lock+LevelThree]= 3; map[Shift+Lock+LevelThree]= 4;
    };
    type "FOUR_LEVEL_KEYPAD" {
        modifiers= Shift+NumLock+LevelThree;
        map[Shift]= 1; map[NumLock]= 2; map[LevelThree]= 3; map[Shift+NumLock]= 2;
        map[Shift+LevelThree]= 4; map[NumLock+LevelThree]= 3; map[Shift+NumLock+LevelThree]= 4;
    };
};
"#;

static BUILTIN_KEY_TYPES: LazyLock<HashMap<String, KeyType>> =
    LazyLock::new(|| parse_keymap(BUILTIN_TYPES).types);

/// X11 keysym names with an exact Unicode mapping (from `keysymdef.h`).
static KEYSYM_TEXT: LazyLock<HashMap<&'static str, char>> = LazyLock::new(|| {
    include_str!("xkb_keysyms.txt")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let (name, value) = line.split_once(' ')?;
            Some((name, char::from_u32(u32::from_str_radix(value, 16).ok()?)?))
        })
        .collect()
});

/// One XKB key type: the modifiers it consumes and their level mapping.
#[derive(Debug, Clone, Default)]
struct KeyType {
    modifiers: u8,
    /// Exact modifier combination to zero-based level.
    map: Vec<(u8, usize)>,
}

impl KeyType {
    /// XKB semantics: mask the active modifiers with the type's modifiers
    /// and look the result up exactly; an unmapped combination is level 1.
    fn level(&self, active: u8) -> usize {
        let effective = active & self.modifiers;
        self.map
            .iter()
            .find(|(modifiers, _)| *modifiers == effective)
            .map_or(0, |(_, level)| *level)
    }
}

#[derive(Debug, Default)]
struct ParsedKeymap {
    keycodes: KeycodeMap,
    symbols: SymbolGroupMap,
    key_types: KeyTypeMap,
    types: HashMap<String, KeyType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rmlvo {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: Option<String>,
    pub options: Option<String>,
}

impl Rmlvo {
    /// Read an explicit layout from the environment.
    ///
    /// We require a layout override rather than silently compiling `us`: a
    /// guessed map could make a physical evdev event look more certain than
    /// the compositor's actual layout.
    pub fn from_environment() -> Option<Self> {
        let layout = env_value("WHYKEY_XKB_LAYOUT", "XKB_DEFAULT_LAYOUT")?;
        if layout.trim().is_empty() {
            return None;
        }
        Some(Self {
            rules: env_value_or("WHYKEY_XKB_RULES", "XKB_DEFAULT_RULES", "evdev"),
            model: env_value_or("WHYKEY_XKB_MODEL", "XKB_DEFAULT_MODEL", "pc105"),
            layout,
            variant: env_value("WHYKEY_XKB_VARIANT", "XKB_DEFAULT_VARIANT"),
            options: env_value("WHYKEY_XKB_OPTIONS", "XKB_DEFAULT_OPTIONS"),
        })
    }
}

fn env_value(primary: &str, fallback: &str) -> Option<String> {
    env::var(primary)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var(fallback)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn env_value_or(primary: &str, fallback: &str, default: &str) -> String {
    env_value(primary, fallback).unwrap_or_else(|| default.into())
}
/// A Linux evdev keycode as reported by the kernel input layer.
///
/// A compiled XKB keymap addresses keys eight higher by convention, so the
/// `+8` shift lives in [`EvdevKeycode::to_xkb`] alone. Constructing this
/// type from a `u16` is lossless; interpreting it as an XKB keycode without
/// conversion is a namespace error the type system now rejects.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct EvdevKeycode(u16);

/// An XKB keycode as used in compiled keymaps, Hyprland socket events, and
/// Hyprland binding fields.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct XkbKeycode(u32);

/// Modifier state needed to choose an XKB symbol level without guessing a
/// layout. The compositor/evdev integration fills this from observed state;
/// absent fields remain unknown rather than silently selecting a US default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct XkbState {
    pub group: Option<usize>,
    pub shift: bool,
    pub altgr: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub latched_shift: bool,
}

impl XkbState {
    /// Build typed state from a compositor modifier bitmask.  Hyprland's
    /// socket uses the stable XKB bits for Shift and AltGr; lock state comes
    /// from the compositor's keyboard snapshot rather than the event mask.
    pub fn from_modifier_mask(
        modifier_mask: u32,
        group: Option<usize>,
        caps_lock: bool,
        num_lock: bool,
        latched_shift: bool,
    ) -> Self {
        Self {
            group,
            shift: modifier_mask & 1 != 0,
            altgr: modifier_mask & 128 != 0,
            caps_lock,
            num_lock,
            latched_shift,
        }
    }

    /// The modifiers a key type can consume, as this state observes them.
    /// A latched Shift is active for the next key exactly like a held one.
    fn type_modifiers(&self) -> u8 {
        let mut modifiers = 0;
        if self.shift || self.latched_shift {
            modifiers |= TYPE_SHIFT;
        }
        if self.caps_lock {
            modifiers |= TYPE_LOCK;
        }
        if self.altgr {
            modifiers |= TYPE_LEVEL_THREE;
        }
        if self.num_lock {
            modifiers |= TYPE_NUM_LOCK;
        }
        modifiers
    }
}

impl EvdevKeycode {
    pub fn new(code: u16) -> Self {
        Self(code)
    }
    pub fn get(self) -> u16 {
        self.0
    }
    /// Convert to the XKB namespace once, at the entry boundary.
    pub fn to_xkb(self) -> Option<XkbKeycode> {
        u32::from(self.0).checked_add(8).map(XkbKeycode)
    }
}

impl XkbKeycode {
    pub fn new(code: u32) -> Self {
        Self(code)
    }
    pub fn get(self) -> u32 {
        self.0
    }
    /// Convert back to the evdev namespace. Wire values below 8 have no
    /// evdev counterpart and yield `None` instead of wrapping.
    pub fn to_evdev(self) -> Option<EvdevKeycode> {
        self.0
            .checked_sub(8)
            .and_then(|code| u16::try_from(code).ok().map(EvdevKeycode))
    }
}

impl From<u16> for EvdevKeycode {
    fn from(code: u16) -> Self {
        Self(code)
    }
}

impl From<u32> for XkbKeycode {
    fn from(code: u32) -> Self {
        Self(code)
    }
}

impl std::fmt::Display for EvdevKeycode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for XkbKeycode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

pub fn compile_keymap(rmlvo: &Rmlvo) -> Option<String> {
    let mut command = Command::new("xkbcli");
    command.args([
        "compile-keymap",
        "--include-defaults",
        "--rules",
        &rmlvo.rules,
        "--model",
        &rmlvo.model,
        "--layout",
        &rmlvo.layout,
    ]);
    if let Some(variant) = rmlvo.variant.as_deref() {
        command.args(["--variant", variant]);
    }
    if let Some(options) = rmlvo.options.as_deref() {
        command.args(["--options", options]);
    }
    let output = command::output(&mut command).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn compile_keymap_from_environment() -> Option<String> {
    compile_keymap(&Rmlvo::from_environment()?)
}

/// Read an optional zero-based active group supplied by an integration or a
/// test harness. A compositor remains the authoritative source when it can
/// expose this state; evdev has no standard way to query it itself.
pub fn group_from_environment() -> usize {
    env::var("WHYKEY_XKB_GROUP")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
}

/// Return the symbols in one XKB group for a Linux evdev key code.
pub fn symbols_for_evdev_keycode(
    keymap: &str,
    evdev_keycode: EvdevKeycode,
    group: usize,
) -> Vec<String> {
    // A compiled XKB keymap uses XKB keycodes. Linux evdev keycodes are eight
    // lower by convention. Do not inspect the raw evdev number too: evdev 28
    // is Return while XKB 28 is the T key.
    let Some(xkb_keycode) = evdev_keycode.to_xkb() else {
        return Vec::new();
    };
    let mut symbols = parse_symbol_keycodes_for_group(keymap, group)
        .into_iter()
        .filter_map(|(symbol, codes)| codes.contains(&xkb_keycode.get()).then_some(symbol))
        .collect::<Vec<_>>();
    symbols.sort();
    symbols.dedup();
    symbols
}

/// Select one symbol level for a physical key while retaining XKB's declared
/// level order. Callers can apply their observed Shift/Caps/AltGr state
/// without losing the distinction between unshifted and shifted symbols.
pub fn preferred_symbol_for_xkb_keycode(
    keymap: &str,
    xkb_keycode: XkbKeycode,
    group: usize,
    level: usize,
) -> Option<String> {
    let parsed = parse_keymap(keymap);
    parsed.keycodes.into_iter().find_map(|(name, codes)| {
        if !codes.contains(&xkb_keycode.get()) {
            return None;
        }
        let levels = parsed.symbols.get(&name)?.get(&group)?;
        levels
            .get(level)
            .or_else(|| levels.first())
            .cloned()
            .filter(|symbol| !symbol.is_empty())
    })
}

pub fn preferred_symbol_for_evdev_keycode(
    keymap: &str,
    evdev_keycode: EvdevKeycode,
    group: usize,
    level: usize,
) -> Option<String> {
    let xkb_keycode = evdev_keycode.to_xkb()?;
    preferred_symbol_for_xkb_keycode(keymap, xkb_keycode, group, level)
}

/// One key's symbol after key-type-aware XKB level selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelSelection {
    /// `None` when the selected level holds `NoSymbol` or does not exist.
    pub keysym: Option<String>,
    /// Zero-based level the key type selects.
    pub level: usize,
    pub key_type: String,
    /// Whether the key type maps LevelThree (AltGr). When it does not, AltGr
    /// selects nothing on this key and stays an unconsumed modifier.
    pub consumes_level_three: bool,
}

/// Select the level XKB would use for `xkb_keycode` under `state`, using the
/// key's type: CapsLock only affects types that map Lock (ALPHABETIC and its
/// four-level variants), and Shift+AltGr reaches level 4 on FOUR_LEVEL*
/// types. `None` when the group is unknown or the key or its type is not in
/// the keymap.
pub fn select_level(
    keymap: &str,
    xkb_keycode: XkbKeycode,
    state: &XkbState,
) -> Option<LevelSelection> {
    select_level_in(&parse_keymap(keymap), xkb_keycode, state)
}

fn select_level_in(
    parsed: &ParsedKeymap,
    xkb_keycode: XkbKeycode,
    state: &XkbState,
) -> Option<LevelSelection> {
    let group = state.group?;
    let name = parsed
        .keycodes
        .iter()
        .find_map(|(name, codes)| codes.contains(&xkb_keycode.get()).then_some(name))?;
    let levels = parsed.symbols.get(name)?.get(&group)?;
    let explicit = parsed
        .key_types
        .get(name)
        .and_then(|types| types.get(&Some(group)).or_else(|| types.get(&None)));
    let type_name = match explicit {
        Some(type_name) => type_name.as_str(),
        None => automatic_type(levels)?,
    };
    let key_type = parsed
        .types
        .get(type_name)
        .or_else(|| BUILTIN_KEY_TYPES.get(type_name))?;
    let level = key_type.level(state.type_modifiers());
    Some(LevelSelection {
        keysym: levels
            .get(level)
            .filter(|symbol| !symbol.is_empty())
            .cloned(),
        level,
        key_type: type_name.to_owned(),
        consumes_level_three: key_type.modifiers & TYPE_LEVEL_THREE != 0,
    })
}

/// The type libxkbcommon assigns to a key group without an explicit `type=`
/// (xkbcomp's `FindAutomaticType`). Compiled keymaps omit those implicit
/// assignments, so the level-1/2 (and 3/4) case pairs decide whether
/// CapsLock applies.
fn automatic_type(levels: &[String]) -> Option<&'static str> {
    let symbol = |index: usize| levels.get(index).map_or("", String::as_str);
    let case_pair = |lower: usize, upper: usize| {
        keysym_is_lower(symbol(lower)) && keysym_is_upper(symbol(upper))
    };
    let keypad = || symbol(0).starts_with("KP_") || symbol(1).starts_with("KP_");
    Some(match levels.len() {
        0 | 1 => "ONE_LEVEL",
        2 if case_pair(0, 1) => "ALPHABETIC",
        2 if keypad() => "KEYPAD",
        2 => "TWO_LEVEL",
        3 | 4 if case_pair(0, 1) && case_pair(2, 3) => "FOUR_LEVEL_ALPHABETIC",
        3 | 4 if case_pair(0, 1) => "FOUR_LEVEL_SEMIALPHABETIC",
        3 | 4 if keypad() => "FOUR_LEVEL_KEYPAD",
        3 | 4 => "FOUR_LEVEL",
        _ => return None,
    })
}

/// The character a keysym commits, for named keysyms with an exact Unicode
/// mapping and for Unicode keysyms (`U20AC`, `0x10020ac`).
pub fn keysym_char(keysym: &str) -> Option<char> {
    if let Some(character) = KEYSYM_TEXT.get(keysym) {
        return Some(*character);
    }
    let value = match keysym.strip_prefix('U') {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => u32::from_str_radix(keysym.strip_prefix("0x")?, 16)
            .ok()?
            .checked_sub(0x0100_0000)?,
    };
    char::from_u32(value)
}

fn single_char(mut characters: impl Iterator<Item = char>) -> Option<char> {
    let character = characters.next()?;
    characters.next().is_none().then_some(character)
}

fn keysym_is_lower(keysym: &str) -> bool {
    keysym_char(keysym).is_some_and(|character| {
        single_char(character.to_uppercase()).is_some_and(|upper| upper != character)
    })
}

fn keysym_is_upper(keysym: &str) -> bool {
    keysym_char(keysym).is_some_and(|character| {
        single_char(character.to_lowercase()).is_some_and(|lower| lower != character)
    })
}

/// The compiled active keymap and layout group, plus the physical key when
/// a capture observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardLayout {
    pub keymap: String,
    pub group: usize,
    pub physical_keycode: Option<XkbKeycode>,
}

/// What a key commits once XKB has consumed its level modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelText {
    Text {
        keysym: String,
        text: char,
    },
    /// A dead keysym starts composition; nothing is committed yet.
    Dead {
        keysym: String,
    },
    /// The text cannot be resolved; the reason names the missing evidence.
    Unresolved(String),
}

/// Resolve the text `key` commits under `state` in `layout`. `key` is a
/// normalized query key (see `key::normalize_key_str`); a captured physical
/// keycode takes precedence over matching it against the base level.
pub fn resolve_text(layout: &KeyboardLayout, key: &str, state: &XkbState) -> LevelText {
    let parsed = parse_keymap(&layout.keymap);
    let keycodes = match layout.physical_keycode {
        Some(keycode) => vec![keycode],
        None => keycodes_for_key(&parsed, layout.group, key),
    };
    let mut outcomes = keycodes
        .iter()
        .map(|keycode| level_text(&parsed, *keycode, key, state));
    let Some(first) = outcomes.next() else {
        return LevelText::Unresolved(format!(
            "{key} is not on the base level of the active XKB layout group {}",
            layout.group + 1
        ));
    };
    if outcomes.all(|outcome| outcome == first) {
        first
    } else {
        LevelText::Unresolved(format!(
            "{key} is on several physical keys that select different symbols"
        ))
    }
}

fn level_text(
    parsed: &ParsedKeymap,
    keycode: XkbKeycode,
    key: &str,
    state: &XkbState,
) -> LevelText {
    let Some(selection) = select_level_in(parsed, keycode, state) else {
        return LevelText::Unresolved(format!(
            "the XKB key type of {key} (keycode {keycode}) is not known"
        ));
    };
    if state.altgr && !selection.consumes_level_three {
        return LevelText::Unresolved(format!(
            "the active layout has no level 3 for {key} (key type {}); AltGr stays an \
             unconsumed modifier",
            selection.key_type
        ));
    }
    let level = selection.level + 1;
    let Some(keysym) = selection.keysym else {
        return LevelText::Unresolved(format!(
            "the active layout has no symbol at level {level} for {key}"
        ));
    };
    if keysym.starts_with("dead_") {
        return LevelText::Dead { keysym };
    }
    match keysym_char(&keysym) {
        Some(text) => LevelText::Text { keysym, text },
        None => LevelText::Unresolved(format!(
            "level {level} keysym {keysym} of {key} has no known text"
        )),
    }
}

/// XKB keycodes whose base-level symbol in `group` is the query key.
fn keycodes_for_key(parsed: &ParsedKeymap, group: usize, key: &str) -> Vec<XkbKeycode> {
    let query = single_char(key.chars());
    let mut keycodes = parsed
        .symbols
        .iter()
        .filter(|(_, groups)| {
            groups
                .get(&group)
                .and_then(|levels| levels.first())
                .is_some_and(|base| {
                    crate::key::normalize_key_str(base) == key
                        || query.is_some_and(|query| keysym_char(base) == Some(query))
                })
        })
        .filter_map(|(name, _)| parsed.keycodes.get(name))
        .flatten()
        .map(|code| XkbKeycode(*code))
        .collect::<Vec<_>>();
    keycodes.sort_by_key(|keycode| keycode.get());
    keycodes.dedup();
    keycodes
}

pub fn parse_symbol_keycodes_for_group(
    keymap: &str,
    group_index: usize,
) -> HashMap<String, Vec<u32>> {
    let ParsedKeymap {
        keycodes, symbols, ..
    } = parse_keymap(keymap);
    symbols
        .into_iter()
        .flat_map(|(name, groups)| {
            let codes = keycodes.get(&name).cloned().unwrap_or_default();
            groups
                .get(&group_index)
                .cloned()
                .into_iter()
                .flat_map(move |symbols_for_key| {
                    symbols_for_key
                        .into_iter()
                        .filter(|symbol| !symbol.is_empty())
                        .flat_map({
                            let codes = codes.clone();
                            move |symbol| {
                                codes
                                    .clone()
                                    .into_iter()
                                    .map(move |code| (symbol.clone(), code))
                            }
                        })
                })
        })
        .fold(HashMap::new(), |mut result, (symbol, code)| {
            let values = result.entry(symbol).or_default();
            if !values.contains(&code) {
                values.push(code);
            }
            result
        })
}

fn parse_keymap(keymap: &str) -> ParsedKeymap {
    let mut parsed = ParsedKeymap::default();
    let mut in_keycodes = false;
    let mut in_symbols = false;
    let mut in_types = false;
    let mut pending_symbol_key = None;
    let mut pending_type: Option<(String, KeyType)> = None;

    for line in keymap.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("xkb_") {
            in_keycodes = trimmed.starts_with("xkb_keycodes");
            in_symbols = trimmed.starts_with("xkb_symbols");
            in_types = trimmed.starts_with("xkb_types");
            pending_symbol_key = None;
            pending_type = None;
            continue;
        }
        if in_keycodes {
            if let Some((name, value)) = parse_keycode_line(trimmed) {
                parsed.keycodes.entry(name).or_default().push(value);
            }
        }
        if in_types {
            parse_type_line(trimmed, &mut pending_type, &mut parsed.types);
        }
        if in_symbols {
            if let Some(name) = parse_symbol_key_name(trimmed) {
                // `key <NAME> { ... };` on one line, or the opener of a block.
                let body = trimmed.split_once('{').map_or("", |(_, body)| body);
                if let Some((group, type_name)) = parse_key_type(body) {
                    parsed
                        .key_types
                        .entry(name.clone())
                        .or_default()
                        .insert(group, type_name);
                }
                if let Some(symbols_for_key) = parse_symbol_fragment(body) {
                    let group = parse_symbol_group(body).unwrap_or(0);
                    parsed
                        .symbols
                        .entry(name)
                        .or_default()
                        .insert(group, symbols_for_key);
                    pending_symbol_key = None;
                } else {
                    pending_symbol_key = Some(name);
                }
            } else if let Some(name) = pending_symbol_key.clone() {
                if let Some((group, type_name)) = parse_key_type(trimmed) {
                    parsed
                        .key_types
                        .entry(name)
                        .or_default()
                        .insert(group, type_name);
                } else if let Some(symbols_for_key) = parse_symbol_fragment(trimmed) {
                    let group = parse_symbol_group(trimmed).unwrap_or(0);
                    parsed
                        .symbols
                        .entry(name)
                        .or_default()
                        .insert(group, symbols_for_key);
                } else if trimmed.starts_with('}') {
                    pending_symbol_key = None;
                }
            }
        }
    }

    parsed
}

/// Parse one line of an `xkb_types` section. Statements end in `;`, and the
/// built-in table puts a whole type on one line, so split by statement.
fn parse_type_line(
    line: &str,
    pending: &mut Option<(String, KeyType)>,
    types: &mut HashMap<String, KeyType>,
) {
    let mut rest = line;
    if let Some(name) = rest
        .strip_prefix("type")
        .map(str::trim_start)
        .and_then(|header| header.strip_prefix('"'))
        .and_then(|header| header.split_once('"'))
        .map(|(name, _)| name)
    {
        *pending = Some((name.to_owned(), KeyType::default()));
        rest = rest.split_once('{').map_or("", |(_, body)| body);
    }
    for statement in rest.split(';').map(str::trim) {
        if statement.starts_with('}') {
            if let Some((name, key_type)) = pending.take() {
                types.insert(name, key_type);
            }
            continue;
        }
        let Some((_, key_type)) = pending.as_mut() else {
            continue;
        };
        if let Some(value) = statement
            .strip_prefix("modifiers")
            .and_then(|value| value.trim_start().strip_prefix('='))
        {
            key_type.modifiers = parse_type_modifiers(value);
        } else if let Some((modifiers, level)) = statement
            .strip_prefix("map[")
            .and_then(|entry| entry.split_once(']'))
        {
            let level = level.trim_start().strip_prefix('=').map(str::trim);
            let level = level.map(|value| value.strip_prefix("Level").unwrap_or(value));
            if let Some(level) = level
                .and_then(|value| value.parse::<usize>().ok())
                .and_then(|value| value.checked_sub(1))
            {
                key_type.map.push((parse_type_modifiers(modifiers), level));
            }
        }
    }
}

fn parse_type_modifiers(value: &str) -> u8 {
    value.split('+').map(str::trim).fold(0, |modifiers, name| {
        modifiers
            | match name {
                "" | "none" | "None" => 0,
                "Shift" => TYPE_SHIFT,
                "Lock" => TYPE_LOCK,
                "LevelThree" | "Mod5" => TYPE_LEVEL_THREE,
                "NumLock" | "Mod2" => TYPE_NUM_LOCK,
                _ => TYPE_OTHER,
            }
    })
}

/// Parse a key's `type= "NAME"` or `type[N]= "NAME"` assignment.
fn parse_key_type(statement: &str) -> Option<(Option<usize>, String)> {
    let rest = statement.trim_start().strip_prefix("type")?;
    let (group, rest) = match rest.strip_prefix('[') {
        Some(indexed) => {
            let (index, rest) = indexed.split_once(']')?;
            let index = index.trim();
            let index = index.strip_prefix("Group").unwrap_or(index);
            (Some(index.parse::<usize>().ok()?.checked_sub(1)?), rest)
        }
        None => (None, rest),
    };
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let name = rest.strip_prefix('"')?.split_once('"')?.0;
    Some((group, name.to_owned()))
}

/// Return every symbol-to-keycode mapping, regardless of layout group.
///
/// This is used by symbolic desktop adapters that need to preserve the full
/// candidate set when the compositor does not expose an active group.
pub fn parse_symbol_keycodes(keymap: &str) -> HashMap<String, Vec<u32>> {
    let mut result = HashMap::new();
    let mut groups = 0;
    while groups < 32 {
        let selected = parse_symbol_keycodes_for_group(keymap, groups);
        if selected.is_empty() && groups > 0 {
            break;
        }
        for (symbol, codes) in selected {
            let values = result.entry(symbol).or_insert_with(Vec::new);
            for code in codes {
                if !values.contains(&code) {
                    values.push(code);
                }
            }
        }
        groups += 1;
    }
    result
}

fn parse_keycode_line(line: &str) -> Option<(String, u32)> {
    let (raw_name, raw_value) = line.split_once('=')?;
    let name = raw_name.trim().strip_prefix('<')?.strip_suffix('>')?;
    let value = raw_value.trim().trim_end_matches(';').parse().ok()?;
    Some((name.to_owned(), value))
}

fn parse_symbol_key_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("key <")?;
    let (name, _) = rest.split_once('>')?;
    Some(name.to_owned())
}

fn parse_symbol_fragment(line: &str) -> Option<Vec<String>> {
    let symbols = line.rsplit_once('[')?.1.split_once(']')?.0;
    let symbols = symbols
        .split(',')
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty())
        .map(|symbol| {
            if symbol == "NoSymbol" {
                String::new()
            } else {
                symbol.to_owned()
            }
        })
        .collect::<Vec<_>>();
    (!symbols.is_empty()).then_some(symbols)
}

fn parse_symbol_group(line: &str) -> Option<usize> {
    // Search the whole statement: a one-line key may put `type= ...` (and
    // its `=`) before `symbols[N]=`.
    let start = line.find("symbols[")? + "symbols[".len();
    let end = line[start..].find(']')? + start;
    let index = line[start..end].trim();
    // libxkbcommon <= 1.7 dumps `symbols[GroupN]`; newer ones `symbols[N]`.
    let index = index.strip_prefix("Group").unwrap_or(index);
    index.parse::<usize>().ok()?.checked_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYMAP: &str = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
    <AE01> = 10;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, Q ]
        symbols[2] = [ a, A ]
    };
    key <AE01> {
        symbols[1] = [ 1, exclam ]
    };
};
};
"#;

    #[test]
    fn selects_symbols_by_group_and_evdev_offset() {
        let first = symbols_for_evdev_keycode(KEYMAP, EvdevKeycode::from(16), 0);
        assert_eq!(first, vec!["Q", "q"]);
        let second_layout = symbols_for_evdev_keycode(KEYMAP, EvdevKeycode::from(16), 1);
        assert_eq!(second_layout, vec!["A", "a"]);
        let second = symbols_for_evdev_keycode(KEYMAP, EvdevKeycode::from(2), 0);
        assert_eq!(second, vec!["1", "exclam"]);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(KEYMAP, EvdevKeycode::from(16), 0, 0),
            Some("q".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(KEYMAP, EvdevKeycode::from(16), 0, 1),
            Some("Q".into())
        );
    }

    #[test]
    fn does_not_mix_raw_evdev_and_xkb_namespaces() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD05> = 28;
    <RTRN> = 36;
};
xkb_symbols "pc" {
    key <AD05> { symbols[1] = [ t, T ] };
    key <RTRN> { symbols[1] = [ Return ] };
};
};
"#;

        assert_eq!(
            symbols_for_evdev_keycode(keymap, EvdevKeycode::from(28), 0),
            vec!["Return"]
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, EvdevKeycode::from(28), 0, 0),
            Some("Return".into())
        );
    }

    #[test]
    fn ignores_unknown_groups() {
        assert!(parse_symbol_keycodes_for_group(KEYMAP, 4).is_empty());
    }

    #[test]
    fn parses_group_prefixed_symbol_indices() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
    <AE01> = 10;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[Group1] = [ q, Q ],
        symbols[Group2] = [ a, A ]
    };
    key <AE01> { type= "TWO_LEVEL", symbols[Group1]= [ 1, exclam ] };
};
};
"#;
        let first = parse_symbol_keycodes_for_group(keymap, 0);
        let second = parse_symbol_keycodes_for_group(keymap, 1);
        assert_eq!(first.get("q"), Some(&vec![24]));
        assert_eq!(first.get("1"), Some(&vec![10]));
        assert!(!first.contains_key("a"));
        assert_eq!(second.get("a"), Some(&vec![24]));
        assert!(!second.contains_key("q"));
    }

    #[test]
    fn preserves_no_symbol_level_positions() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
};
xkb_symbols "pc" {
    key <AD01> {
        symbols[1] = [ q, NoSymbol, Q ]
    };
};
};
"#;
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, EvdevKeycode::from(16), 0, 0),
            Some("q".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, EvdevKeycode::from(16), 0, 1),
            None
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, EvdevKeycode::from(16), 0, 2),
            Some("Q".into())
        );
        let symbols = symbols_for_evdev_keycode(keymap, EvdevKeycode::from(16), 0);
        assert_eq!(symbols, vec!["Q", "q"]);
    }

    #[test]
    fn evdev_converts_to_xkb_once_with_plus_eight() {
        assert_eq!(EvdevKeycode::from(28).to_xkb(), Some(XkbKeycode::from(36)));
        assert_eq!(
            XkbKeycode::from(36).to_evdev(),
            Some(EvdevKeycode::from(28))
        );
        // An evdev 28 event (Return) must never equal XKB 28 (the T key).
        assert_ne!(EvdevKeycode::from(28).to_xkb(), Some(XkbKeycode::from(28)));
        // Wire values below the offset have no evdev counterpart.
        assert_eq!(XkbKeycode::from(7).to_evdev(), None);
        // Serialization stays numeric for report compatibility.
        assert_eq!(
            serde_json::to_string(&EvdevKeycode::from(28)).unwrap(),
            "28"
        );
        assert_eq!(serde_json::to_string(&XkbKeycode::from(36)).unwrap(), "36");
    }

    #[test]
    fn resolves_shift_altgr_and_numlock_levels() {
        let keymap = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AD01> = 24;
    <AE05> = 14;
    <KP1> = 87;
};
xkb_symbols "pc" {
    key <AD01> { symbols[1] = [ q, Q ] };
    key <AE05> { symbols[1] = [ 5, percent, EuroSign ] };
    key <KP1> { symbols[1] = [ KP_End, KP_1 ] };
};
};
"#;
        let letter = EvdevKeycode::from(16);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, letter, 0, 0),
            Some("q".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, letter, 0, 1),
            Some("Q".into())
        );
        let digit = EvdevKeycode::from(6);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, digit, 0, 1),
            Some("percent".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, digit, 0, 2),
            Some("EuroSign".into())
        );
        // NumLock selects the keypad level: KP_End without it, KP_1 with it.
        let keypad = EvdevKeycode::from(79);
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, keypad, 0, 0),
            Some("KP_End".into())
        );
        assert_eq!(
            preferred_symbol_for_evdev_keycode(keymap, keypad, 0, 1),
            Some("KP_1".into())
        );
    }

    const TYPED_KEYMAP: &str = r#"
xkb_keymap {
xkb_keycodes "evdev" {
    <AE01> = 10;
    <AE02> = 11;
    <AD01> = 24;
    <AD02> = 25;
    <AC10> = 47;
    <RTRN> = 36;
    <KP1> = 87;
    <FK01> = 67;
};
xkb_types "complete" {
    virtual_modifiers NumLock,Alt,LevelThree;
    type "TWO_LEVEL" {
        modifiers= Shift;
        map[Shift]= 2;
    };
    type "CTRL+ALT" {
        modifiers= Shift+Control+Alt+LevelThree;
        map[Shift]= 2;
        map[LevelThree]= 3;
        map[Shift+LevelThree]= 4;
        map[Control+Alt]= 5;
    };
};
xkb_symbols "pc" {
    key <AE01> { [ 1, exclam, exclamdown, onesuperior ] };
    key <AE02> { [ 2, at, twosuperior, dead_doubleacute ] };
    key <AD01> { [ q, Q, adiaeresis, Adiaeresis ] };
    key <AD02> {
        type= "TWO_LEVEL",
        symbols[1]= [ w, W ]
    };
    key <AC10> { [ semicolon, colon, paragraph, degree ] };
    key <RTRN> { [ Return ] };
    key <KP1> { [ KP_End, KP_1 ] };
    key <FK01> {
        type= "CTRL+ALT",
        symbols[1]= [ F1, F1, F1, F1, XF86Switch_VT_1 ]
    };
};
};
"#;

    fn symbol(keycode: u32, state: XkbState) -> Option<String> {
        select_level(TYPED_KEYMAP, XkbKeycode::from(keycode), &state).and_then(|s| s.keysym)
    }

    fn state(shift: bool, caps_lock: bool, altgr: bool) -> XkbState {
        XkbState {
            group: Some(0),
            shift,
            altgr,
            caps_lock,
            ..XkbState::default()
        }
    }

    #[test]
    fn caps_lock_only_affects_keys_whose_type_maps_lock() {
        // Digits and punctuation are FOUR_LEVEL: CapsLock leaves them alone.
        assert_eq!(symbol(10, state(false, true, false)), Some("1".into()));
        assert_eq!(
            symbol(47, state(false, true, false)),
            Some("semicolon".into())
        );
        // Letters are FOUR_LEVEL_ALPHABETIC: CapsLock selects level 2 and
        // Shift cancels it.
        assert_eq!(symbol(24, state(false, true, false)), Some("Q".into()));
        assert_eq!(symbol(24, state(true, true, false)), Some("q".into()));
        // An explicit type from the keymap beats the case-pair inference.
        assert_eq!(symbol(25, state(false, true, false)), Some("w".into()));
        assert_eq!(symbol(25, state(true, false, false)), Some("W".into()));
    }

    #[test]
    fn shift_with_altgr_selects_level_four_on_four_level_types() {
        assert_eq!(
            symbol(24, state(false, false, true)),
            Some("adiaeresis".into())
        );
        assert_eq!(
            symbol(24, state(true, false, true)),
            Some("Adiaeresis".into())
        );
        assert_eq!(
            symbol(10, state(true, false, true)),
            Some("onesuperior".into())
        );
        // CapsLock reaches level 4 on alphabetic types only.
        assert_eq!(
            symbol(24, state(false, true, true)),
            Some("Adiaeresis".into())
        );
        assert_eq!(
            symbol(47, state(false, true, true)),
            Some("paragraph".into())
        );
        // Keymap-defined types are honored too.
        assert_eq!(symbol(67, state(true, false, true)), Some("F1".into()));
    }

    #[test]
    fn typed_state_keeps_missing_group_conditional_and_selects_latched_and_keypad_levels() {
        assert_eq!(
            select_level(TYPED_KEYMAP, XkbKeycode::from(24), &XkbState::default()),
            None
        );
        let latched = XkbState {
            group: Some(0),
            latched_shift: true,
            ..XkbState::default()
        };
        assert_eq!(symbol(24, latched), Some("Q".into()));
        let num_lock = XkbState {
            group: Some(0),
            num_lock: true,
            ..XkbState::default()
        };
        assert_eq!(symbol(87, num_lock), Some("KP_1".into()));
        assert_eq!(symbol(87, state(true, false, false)), Some("KP_End".into()));
        // AltGr on a one-level key selects nothing and is not consumed.
        let selection = select_level(
            TYPED_KEYMAP,
            XkbKeycode::from(36),
            &state(false, false, true),
        )
        .unwrap();
        assert_eq!(selection.keysym.as_deref(), Some("Return"));
        assert!(!selection.consumes_level_three);
    }

    #[test]
    fn resolves_altgr_text_dead_keys_and_missing_levels() {
        let layout = KeyboardLayout {
            keymap: TYPED_KEYMAP.into(),
            group: 0,
            physical_keycode: None,
        };
        assert_eq!(
            resolve_text(&layout, "Q", &state(false, false, true)),
            LevelText::Text {
                keysym: "adiaeresis".into(),
                text: 'ä'
            }
        );
        assert_eq!(
            resolve_text(&layout, "2", &state(true, false, true)),
            LevelText::Dead {
                keysym: "dead_doubleacute".into()
            }
        );
        for key in ["RETURN", "F1", "Z"] {
            assert!(
                matches!(
                    resolve_text(&layout, key, &state(false, false, true)),
                    LevelText::Unresolved(_)
                ),
                "{key}"
            );
        }
        // A captured physical key wins over matching the query symbol.
        let physical = KeyboardLayout {
            physical_keycode: Some(XkbKeycode::from(10)),
            ..layout
        };
        assert_eq!(
            resolve_text(&physical, "Q", &state(false, false, true)),
            LevelText::Text {
                keysym: "exclamdown".into(),
                text: '¡'
            }
        );
    }

    #[test]
    fn compositor_modifier_mask_populates_typed_state_without_guessing_group() {
        let state = XkbState::from_modifier_mask(1 | 128, None, true, false, false);
        assert_eq!(state.group, None);
        assert!(state.shift);
        assert!(state.altgr);
        assert!(state.caps_lock);
        assert_eq!(
            select_level(TYPED_KEYMAP, XkbKeycode::from(24), &state),
            None
        );
    }

    #[test]
    fn resolves_level_three_text_from_an_xkbcli_compiled_keymap() {
        let rmlvo = Rmlvo {
            rules: "evdev".into(),
            model: "pc105".into(),
            layout: "us".into(),
            variant: Some("intl".into()),
            options: None,
        };
        let Some(keymap) = compile_keymap(&rmlvo) else {
            // xkbcli is optional; the fixtures above stay authoritative.
            return;
        };
        let layout = KeyboardLayout {
            keymap,
            group: 0,
            physical_keycode: None,
        };
        assert_eq!(
            resolve_text(&layout, "Q", &state(false, false, true)),
            LevelText::Text {
                keysym: "adiaeresis".into(),
                text: 'ä'
            }
        );
        assert_eq!(
            resolve_text(&layout, "1", &state(false, true, false)),
            LevelText::Text {
                keysym: "1".into(),
                text: '1'
            }
        );
        assert!(matches!(
            resolve_text(&layout, "F1", &state(false, false, true)),
            LevelText::Unresolved(_)
        ));
    }

    #[test]
    fn parses_the_wire_shape_emitted_by_xkbcli_for_multiple_layouts() {
        let rmlvo = Rmlvo {
            rules: "evdev".into(),
            model: "pc105".into(),
            layout: "us,de".into(),
            variant: None,
            options: None,
        };
        let Some(keymap) = compile_keymap(&rmlvo) else {
            // xkbcli is an optional runtime dependency on systems without
            // libxkbcommon tools; the static parser fixture above remains
            // authoritative in that environment.
            return;
        };
        let us = symbols_for_evdev_keycode(&keymap, EvdevKeycode::from(21), 0);
        let german = symbols_for_evdev_keycode(&keymap, EvdevKeycode::from(21), 1);
        assert!(us.iter().any(|symbol| symbol == "y"));
        assert!(german.iter().any(|symbol| symbol == "z"));
    }
}
