//! Static evaluation of input-remapper (v2) presets: the preset each device
//! autoloads (`autoload` in `config.json`), its EV_KEY mappings in the
//! current list format or the older `mapping` dictionary, and output
//! symbols resolved the way input-remapper resolves them: the session's
//! `xmodmap.json` symbols, then Linux `KEY_` names. `disable` and macros
//! consume the key; combinations and timing-dependent macros stay
//! conditional.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::keys::{self, Press};
use super::{DeviceScope, Effect, Finding, Rule, same_device_name};

const EV_KEY: u64 = 1;
/// Macro functions whose result depends on timing or other state.
const CONDITIONAL_MACROS: &[&str] = &[
    "if_tap",
    "if_single",
    "if_capslock",
    "if_numlock",
    "if_eq",
    "ifeq",
    "mod_tap",
];

/// The `(device, preset)` pairs input-remapper autoloads.
pub(crate) fn autoload(content: &str) -> Result<Vec<(String, String)>, String> {
    let value: Value = serde_json::from_str(content).map_err(|error| error.to_string())?;
    let Some(config) = value.as_object() else {
        return Err("the configuration is not a JSON object".into());
    };
    let Some(autoload) = config.get("autoload") else {
        return Ok(Vec::new());
    };
    let Some(entries) = autoload.as_object() else {
        return Err("autoload is not a JSON object".into());
    };
    Ok(entries
        .iter()
        .filter_map(|(device, preset)| Some((device.clone(), preset.as_str()?.to_owned())))
        .collect())
}

/// One device group input-remapper forms, with its autoload key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputGroup {
    key: String,
    /// The shortest device name, which names the preset directory.
    name: String,
    names: Vec<String>,
}

/// The device groups input-remapper forms (groups.py `_FindGroups`),
/// reproduced from `/proc/bus/input/devices`: evdev nodes sorted by path
/// and then by `get_unique_key` (bus, vendor, product, uniq, and phys up
/// to its first `/`), without input-remapper's own devices, the
/// denylisted ones, cameras, `Power Button`, and devices without key or
/// axis events. A group is named after its shortest device name and keyed
/// by that name, or `<name> 2`, `<name> 3`, … when the name is taken.
pub(crate) fn present_groups(devices: &str) -> Vec<InputGroup> {
    let mut nodes: Vec<(String, String, String)> = devices
        .split("\n\n")
        .filter_map(input_remapper_node)
        .collect();
    nodes.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut grouped: Vec<(&str, Vec<String>)> = Vec::new();
    for (unique, _, name) in &nodes {
        match grouped.iter_mut().find(|(other, _)| other == unique) {
            Some((_, names)) => names.push(name.clone()),
            None => grouped.push((unique, vec![name.clone()])),
        }
    }
    let mut groups: Vec<InputGroup> = Vec::new();
    for (_, names) in grouped {
        // Python's `min(names, key=len)`: the first of the shortest.
        let Some(name) = names.iter().reduce(|shortest, name| {
            if name.chars().count() < shortest.chars().count() {
                name
            } else {
                shortest
            }
        }) else {
            continue;
        };
        let name = name.clone();
        let mut key = name.clone();
        let mut number = 2;
        while groups.iter().any(|group| group.key == key) {
            key = format!("{name} {number}");
            number += 1;
        }
        groups.push(InputGroup { key, name, names });
    }
    groups
}

/// `(unique key, device path, name)` of one `/proc/bus/input/devices`
/// entry that input-remapper groups.
fn input_remapper_node(block: &str) -> Option<(String, String, String)> {
    const EV_REL: usize = 2;
    const EV_ABS: usize = 3;
    const KEY_CAMERA: usize = 212;
    const ABS_MT_POSITION_X: usize = 0x35;
    let field = |prefix: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
    };
    let info = field("I: ")?;
    let id = |label: &str| {
        info.split_whitespace()
            .find_map(|field| field.strip_prefix(label))
            .and_then(|hex| u16::from_str_radix(hex, 16).ok())
    };
    let name = field("N: Name=")?.trim_matches('"');
    let phys = field("P: Phys=").unwrap_or_default();
    let uniq = field("U: Uniq=").unwrap_or_default();
    let node = field("H: Handlers=")?
        .split_whitespace()
        .find(|handler| handler.starts_with("event"))?;
    let events = bitmap(field("B: EV=").unwrap_or_default());
    let keys = bitmap(field("B: KEY=").unwrap_or_default());
    let axes = bitmap(field("B: ABS=").unwrap_or_default());
    let camera = keys == [KEY_CAMERA] && !axes.contains(&ABS_MT_POSITION_X);
    let useful = [EV_KEY as usize, EV_REL, EV_ABS]
        .iter()
        .any(|kind| events.contains(kind));
    let lowercase = name.to_lowercase();
    let denylisted = lowercase.starts_with("eee pc wmi hotkeys")
        || lowercase
            .find("yubico")
            .is_some_and(|start| lowercase[start..].contains("yubikey"));
    if name == "Power Button"
        || name.starts_with("input-remapper")
        || phys.starts_with("input-remapper")
        || camera
        || !useful
        || denylisted
    {
        return None;
    }
    let unique = format!(
        "{}_{}_{}_{uniq}_{}",
        id("Bus=")?,
        id("Vendor=")?,
        id("Product=")?,
        match phys.split('/').next() {
            Some(port) if !port.is_empty() => port,
            _ => "-",
        }
    );
    Some((unique, format!("/dev/input/{node}"), name.to_owned()))
}

/// The set bits of a `/proc/bus/input/devices` bitmap: hexadecimal
/// `unsigned long` words, most significant first.
fn bitmap(words: &str) -> Vec<usize> {
    let width = usize::BITS as usize;
    words
        .split_whitespace()
        .rev()
        .enumerate()
        .flat_map(|(index, word)| {
            let word = u64::from_str_radix(word, 16).unwrap_or_default();
            (0..width)
                .filter(move |bit| (word >> bit) & 1 != 0)
                .map(move |bit| index * width + bit)
        })
        .collect()
}

/// The group name behind an `autoload` key, which names the preset
/// directory, and the devices the preset reads. Device names that another
/// present group shares do not identify the group. An absent group, or
/// unknown `groups`, leaves the key as the name.
pub(crate) fn resolve_group(key: &str, groups: Option<&[InputGroup]>) -> (String, DeviceScope) {
    let Some(group) = groups.and_then(|groups| groups.iter().find(|group| group.key == key)) else {
        return (key.to_owned(), DeviceScope::named(key));
    };
    let others = groups.unwrap_or_default();
    let names = group
        .names
        .iter()
        .filter(|name| {
            !others.iter().any(|other| {
                other.key != group.key
                    && other
                        .names
                        .iter()
                        .any(|shared| same_device_name(shared, name))
            })
        })
        .cloned()
        .collect();
    (
        group.name.clone(),
        DeviceScope {
            label: key.to_owned(),
            names,
        },
    )
}

/// `presets/<device>/<preset>.json`, with the characters input-remapper
/// replaces in the device name.
pub(crate) fn preset_path(dir: &Path, device: &str, preset: &str) -> PathBuf {
    let device: String = device
        .chars()
        .map(|character| {
            if "/\\?%*:|\"<>".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let file = if preset.ends_with(".json") {
        preset.to_owned()
    } else {
        format!("{preset}.json")
    };
    dir.join("presets").join(device).join(file)
}

/// The symbol names of the session keyboard layout that input-remapper
/// stores in `xmodmap.json`, with their evdev codes.
pub(crate) fn layout_symbols(content: &str) -> Result<Vec<(String, u16)>, String> {
    let value: Value = serde_json::from_str(content).map_err(|error| error.to_string())?;
    let Some(symbols) = value.as_object() else {
        return Err("xmodmap.json is not a JSON object".into());
    };
    Ok(symbols
        .iter()
        .filter_map(|(name, code)| {
            let code = u16::try_from(code.as_u64()?).ok()?;
            Some((name.clone(), code))
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Output {
    /// The key press emitted; held modifiers carry through.
    Key(Press),
    Effect(Effect),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mapping {
    /// EV_KEY codes of the input combination.
    keys: Vec<u16>,
    /// The combination also needs a non-key event (axis, wheel).
    other_inputs: bool,
    output: Output,
    rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preset {
    mappings: Vec<Mapping>,
    device: DeviceScope,
}

/// Parse one preset that input-remapper applies to `device`. `symbols` are
/// the session's `xmodmap.json` names, when that file exists.
pub(crate) fn load(
    path: &Path,
    content: &str,
    device: &DeviceScope,
    symbols: Option<&[(String, u16)]>,
) -> Result<Preset, String> {
    let display = path.display().to_string();
    let value: Value =
        serde_json::from_str(content).map_err(|error| format!("{display}: {error}"))?;
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let label = format!("input-remapper preset {name} for {}", device.label);
    let mut mappings = Vec::new();
    match &value {
        Value::Array(entries) => {
            for entry in entries {
                let Some(combination) = entry.get("input_combination").and_then(Value::as_array)
                else {
                    continue;
                };
                let events = combination.iter().filter_map(|event| {
                    Some((event.get("type")?.as_u64()?, event.get("code")?.as_u64()?))
                });
                let output = match entry.get("output_symbol").and_then(Value::as_str) {
                    Some(symbol) if !symbol.trim().is_empty() => output(symbol, symbols),
                    _ => match (
                        entry.get("output_type").and_then(Value::as_u64),
                        entry.get("output_code").and_then(Value::as_u64),
                    ) {
                        (Some(EV_KEY), Some(code)) => match u16::try_from(code) {
                            Ok(code) => Output::Key(Press::new(0, code)),
                            Err(_) => continue,
                        },
                        _ => Output::Effect(Effect::Consume(
                            "input-remapper maps it to a non-key event".into(),
                        )),
                    },
                };
                let shown = entry
                    .get("output_symbol")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "type {} code {}",
                            entry.get("output_type").unwrap_or(&Value::Null),
                            entry.get("output_code").unwrap_or(&Value::Null)
                        )
                    });
                mappings.extend(mapping(events, output, &shown, &label, &display));
            }
        }
        Value::Object(object) if object.contains_key("mapping") => {
            let Some(entries) = object.get("mapping").and_then(Value::as_object) else {
                return Err(format!("{display}: mapping is not a JSON object"));
            };
            for (combination, target) in entries {
                let symbol = match target {
                    Value::String(symbol) => symbol.as_str(),
                    Value::Array(pair) => match pair.first().and_then(Value::as_str) {
                        Some(symbol) => symbol,
                        None => continue,
                    },
                    _ => continue,
                };
                let events = combination.split('+').filter_map(|event| {
                    let mut fields = event.split(',').map(|field| field.trim().parse::<u64>());
                    Some((fields.next()?.ok()?, fields.next()?.ok()?))
                });
                mappings.extend(mapping(
                    events,
                    output(symbol, symbols),
                    symbol,
                    &label,
                    &display,
                ));
            }
        }
        _ => return Err(format!("{display}: not an input-remapper preset")),
    }
    Ok(Preset {
        mappings,
        device: device.clone(),
    })
}

fn mapping(
    events: impl Iterator<Item = (u64, u64)>,
    output: Output,
    shown: &str,
    label: &str,
    file: &str,
) -> Option<Mapping> {
    let mut keys = Vec::new();
    let mut other_inputs = false;
    let mut inputs = Vec::new();
    for (kind, code) in events {
        match u16::try_from(code).ok().filter(|_| kind == EV_KEY) {
            Some(code) => {
                keys.push(code);
                inputs.push(keys::evdev_name(code));
            }
            None => {
                other_inputs = true;
                inputs.push(format!("type {kind} code {code}"));
            }
        }
    }
    if keys.is_empty() {
        return None;
    }
    Some(Mapping {
        keys,
        other_inputs,
        output,
        rule: Rule::new(
            file,
            0,
            &format!("{label}: {} -> {}", inputs.join(" + "), shown.trim()),
        ),
    })
}

/// What input-remapper emits for an output symbol.
fn output(symbol: &str, symbols: Option<&[(String, u16)]>) -> Output {
    let symbol = symbol.trim();
    if symbol == "disable" {
        return Output::Effect(Effect::Consume("disabled by input-remapper".into()));
    }
    if symbol.contains('(') && symbol.contains(')') && symbol.len() >= 4 {
        let conditional = macro_functions(symbol).any(|name| CONDITIONAL_MACROS.contains(&name));
        return Output::Effect(if conditional {
            Effect::Conditional(format!(
                "input-remapper macro {symbol} depends on timing or other keys"
            ))
        } else {
            Effect::Consume(format!("input-remapper runs the macro {symbol}"))
        });
    }
    let mut mods = 0;
    let mut code = None;
    let parts: Vec<&str> = symbol.split('+').map(str::trim).collect();
    for part in &parts {
        let Some(emitted) = symbol_code(part, symbols) else {
            return Output::Effect(Effect::Conditional(format!(
                "input-remapper output {part} depends on symbols whykey cannot resolve{}",
                if symbols.is_none() {
                    " (no xmodmap.json)"
                } else {
                    ""
                }
            )));
        };
        match keys::modifier_bit(emitted) {
            Some(bit) if parts.len() > 1 => mods |= bit,
            _ if code.is_none() => code = Some(emitted),
            _ => {
                return Output::Effect(Effect::Consume(format!("input-remapper presses {symbol}")));
            }
        }
    }
    match code {
        Some(code) => Output::Key(Press::new(mods, code)),
        None => Output::Effect(Effect::Consume(format!(
            "input-remapper holds modifiers {symbol}"
        ))),
    }
}

/// Function names called in a macro, e.g. `key` and `modify`.
fn macro_functions(symbol: &str) -> impl Iterator<Item = &str> {
    symbol.split('(').filter_map(|before| {
        let name = before
            .rsplit(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .next()
            .unwrap_or_default();
        (!name.is_empty()).then_some(name)
    })
}

/// input-remapper's symbol lookup: an exact name, then a case-insensitive
/// one. Without `xmodmap.json`, only keysyms every layout places on the
/// same key resolve.
fn symbol_code(symbol: &str, symbols: Option<&[(String, u16)]>) -> Option<u16> {
    if let Some(code) = symbols.and_then(|symbols| {
        symbols
            .iter()
            .find(|(name, _)| name == symbol)
            .map(|(_, code)| *code)
    }) {
        return Some(code);
    }
    if symbol
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("KEY_"))
    {
        return keys::evdev_code(symbol);
    }
    match symbols {
        Some(symbols) => symbols
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(symbol))
            .map(|(_, code)| *code),
        None => keys::keysym_code(symbol),
    }
}

impl Preset {
    /// The preset's effect on `press`. `device` is the captured input
    /// device, when known.
    pub(crate) fn evaluate(&self, press: Press, device: Option<&str>) -> Vec<Finding> {
        let mut findings = Vec::new();
        for bit in keys::modifier_bits(press.mods) {
            let remapped: Vec<&Mapping> = keys::modifier_codes(bit)
                .iter()
                .filter_map(|code| {
                    self.single(*code)
                        .filter(|mapping| mapping.output != Output::Key(Press::new(0, *code)))
                })
                .collect();
            if remapped.len() == keys::modifier_codes(bit).len() {
                findings.push(Finding {
                    effect: Effect::Conditional(
                        "input-remapper remaps the held modifier keys; the modifiers that reach its output are not evaluated"
                            .into(),
                    ),
                    rule: remapped.first().map(|mapping| mapping.rule.clone()),
                });
            }
        }
        if findings.is_empty() {
            for mapping in self
                .mappings
                .iter()
                .filter(|mapping| mapping.keys.contains(&press.code))
            {
                let effect = if mapping.keys.len() > 1 || mapping.other_inputs {
                    Effect::Conditional(
                        "input-remapper maps it only in combination with other inputs".into(),
                    )
                } else {
                    match &mapping.output {
                        Output::Key(emitted) => {
                            Effect::Remap(Press::new(press.mods | emitted.mods, emitted.code))
                        }
                        Output::Effect(effect) => effect.clone(),
                    }
                };
                findings.push(Finding {
                    effect,
                    rule: Some(mapping.rule.clone()),
                });
            }
        }
        if findings.is_empty() {
            findings.push(Finding::untouched());
        }
        if !self.device.includes(device) {
            findings.push(self.device.finding("input-remapper"));
        }
        findings
    }

    fn single(&self, code: u16) -> Option<&Mapping> {
        self.mappings
            .iter()
            .find(|mapping| mapping.keys == [code] && !mapping.other_inputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(content: &str, symbols: Option<&[(String, u16)]>) -> Preset {
        load(
            Path::new("/home/u/.config/input-remapper-2/presets/kbd/main.json"),
            content,
            &DeviceScope::named("kbd"),
            symbols,
        )
        .unwrap()
    }

    fn effects(preset: &Preset, press: Press) -> Vec<Effect> {
        preset
            .evaluate(press, Some("kbd"))
            .into_iter()
            .map(|finding| finding.effect)
            .collect()
    }

    #[test]
    fn list_presets_map_single_keys_combinations_and_macros() {
        let preset = preset(
            r#"[
                {"input_combination": [{"type": 1, "code": 58, "origin_hash": "ab"}], "target_uinput": "keyboard", "output_symbol": "Escape"},
                {"input_combination": [{"type": 1, "code": 30}], "target_uinput": "keyboard", "output_symbol": "Control_L + KEY_B"},
                {"input_combination": [{"type": 1, "code": 31}], "target_uinput": "keyboard", "output_symbol": "key(a).key(b)"},
                {"input_combination": [{"type": 1, "code": 32}], "target_uinput": "keyboard", "output_symbol": "mod_tap(a, Shift_L)"},
                {"input_combination": [{"type": 1, "code": 29}, {"type": 1, "code": 33}], "target_uinput": "keyboard", "output_symbol": "KEY_X"},
                {"input_combination": [{"type": 1, "code": 34}], "target_uinput": "keyboard", "output_symbol": "disable"},
                {"input_combination": [{"type": 1, "code": 35}], "target_uinput": "keyboard", "output_type": 1, "output_code": 2},
                {"input_combination": [{"type": 1, "code": 36}], "target_uinput": "keyboard", "output_symbol": "a"}
            ]"#,
            None,
        );
        assert_eq!(
            effects(&preset, Press::new(keys::SHIFT, 58)),
            vec![Effect::Remap(Press::new(keys::SHIFT, 1))]
        );
        assert_eq!(
            effects(&preset, Press::new(0, 30)),
            vec![Effect::Remap(Press::new(keys::CTRL, 48))]
        );
        assert!(matches!(
            effects(&preset, Press::new(0, 31))[..],
            [Effect::Consume(_)]
        ));
        assert!(matches!(
            effects(&preset, Press::new(0, 32))[..],
            [Effect::Conditional(_)]
        ));
        assert!(matches!(
            effects(&preset, Press::new(0, 33))[..],
            [Effect::Conditional(_)]
        ));
        assert!(matches!(
            effects(&preset, Press::new(0, 34))[..],
            [Effect::Consume(_)]
        ));
        assert_eq!(
            effects(&preset, Press::new(0, 35)),
            vec![Effect::Remap(Press::new(0, 2))]
        );
        // "a" names the key that types it in the session layout.
        assert!(matches!(
            effects(&preset, Press::new(0, 36))[..],
            [Effect::Conditional(_)]
        ));
        let symbols = [("a".to_owned(), 16)];
        let with_layout = preset_with_symbols(&symbols);
        assert_eq!(
            effects(&with_layout, Press::new(0, 36)),
            vec![Effect::Remap(Press::new(0, 16))]
        );
        assert_eq!(effects(&preset, Press::new(0, 59)), vec![Effect::Untouched]);
    }

    fn preset_with_symbols(symbols: &[(String, u16)]) -> Preset {
        preset(
            r#"[{"input_combination": [{"type": 1, "code": 36}], "output_symbol": "A"}]"#,
            Some(symbols),
        )
    }

    #[test]
    fn older_mapping_dictionaries_are_read() {
        let preset = preset(
            r#"{"mapping": {"1,58,1": ["Escape", "keyboard"], "1,30": "KEY_B", "1,29,1+1,31,1": "KEY_C"}}"#,
            None,
        );
        assert_eq!(
            effects(&preset, Press::new(0, 58)),
            vec![Effect::Remap(Press::new(0, 1))]
        );
        assert_eq!(
            effects(&preset, Press::new(0, 30)),
            vec![Effect::Remap(Press::new(0, 48))]
        );
        assert!(matches!(
            effects(&preset, Press::new(0, 31))[..],
            [Effect::Conditional(_)]
        ));
    }

    #[test]
    fn autoload_and_preset_paths() {
        let entries =
            autoload(r#"{"version": "2.0.1", "autoload": {"AT Keyboard: x/y": "work"}}"#).unwrap();
        assert_eq!(entries, vec![("AT Keyboard: x/y".into(), "work".into())]);
        assert_eq!(
            preset_path(Path::new("/c"), &entries[0].0, &entries[0].1),
            Path::new("/c/presets/AT Keyboard_ x_y/work.json")
        );
        assert!(autoload("[1]").is_err());
        assert_eq!(
            layout_symbols(r#"{"a": 30, "Escape": 1}"#).unwrap(),
            vec![("Escape".into(), 1), ("a".into(), 30)]
        );
    }

    #[test]
    fn macros_with_non_ascii_names_do_not_panic() {
        let preset = preset(
            r#"[{"input_combination": [{"type": 1, "code": 31}], "output_symbol": "key(a).é(b)"}]"#,
            None,
        );
        assert_eq!(effects(&preset, Press::new(0, 31)).len(), 1);
        assert_eq!(macro_functions("key(a).é(b)").collect::<Vec<_>>(), ["key"]);
    }

    #[test]
    fn autoload_keys_resolve_to_input_remapper_groups() {
        let device = |bus: &str,
                      vendor: &str,
                      name: &str,
                      phys: &str,
                      uniq: &str,
                      node: u32,
                      key: &str| {
            format!(
                "I: Bus={bus} Vendor={vendor} Product=0001 Version=0111\nN: Name=\"{name}\"\nP: Phys={phys}\nS: Sysfs=/devices/x\nU: Uniq={uniq}\nH: Handlers=kbd event{node}\nB: EV=1f\nB: KEY={key}\n"
            )
        };
        let devices = [
            // Listed first, but port -4 sorts after port -3.
            device(
                "0003",
                "1234",
                "Pad",
                "usb-0000:00:14.0-4/input0",
                "",
                7,
                "1",
            ),
            device(
                "0003",
                "1234",
                "Pad Mouse",
                "usb-0000:00:14.0-4/input1",
                "",
                8,
                "1",
            ),
            device(
                "0003",
                "1234",
                "Pad",
                "usb-0000:00:14.0-3/input0",
                "",
                6,
                "1",
            ),
            device(
                "0003",
                "046d",
                "Logi Receiver",
                "usb-0000:00:14.0-2/input0",
                "",
                5,
                "1",
            ),
            device(
                "0003",
                "046d",
                "Logi Receiver Consumer Control",
                "usb-0000:00:14.0-2/input1",
                "",
                12,
                "1",
            ),
            // Same port; uniq tells the controllers apart.
            device(
                "0005",
                "054c",
                "Wireless Controller",
                "aa:bb",
                "11:22",
                20,
                "1",
            ),
            device(
                "0005",
                "054c",
                "Wireless Controller",
                "aa:bb",
                "33:44",
                21,
                "1",
            ),
            // Equal lengths: the first by device path names the group.
            device("0011", "0001", "XY", "isa0060/serio0/input0", "", 9, "1"),
            device("0011", "0001", "AB", "isa0060/serio0/input1", "", 10, "1"),
            device(
                "0019",
                "0000",
                "Power Button",
                "PNP0C0C/button/input0",
                "",
                1,
                "1",
            ),
            device(
                "0003",
                "0bda",
                "Integrated Camera",
                "usb-0000:00:14.0-8/button",
                "",
                15,
                "100000 0 0 0",
            ),
            device(
                "0006",
                "0001",
                "input-remapper Pad forwarded",
                "",
                "",
                30,
                "1",
            ),
            device(
                "0003",
                "1050",
                "Yubico YubiKey OTP+FIDO",
                "usb-0000:00:14.0-9/input0",
                "",
                31,
                "1",
            ),
        ]
        .join("\n");
        let groups = present_groups(&devices);
        let keys: Vec<_> = groups.iter().map(|group| group.key.as_str()).collect();
        assert_eq!(
            keys,
            // Unique keys sort as strings of decimal ids: "17_…" < "3_…".
            [
                "AB",
                "Logi Receiver",
                "Pad",
                "Pad 2",
                "Wireless Controller",
                "Wireless Controller 2"
            ]
        );
        let (name, scope) = resolve_group("Logi Receiver", Some(&groups));
        assert_eq!(name, "Logi Receiver");
        assert!(scope.includes(Some("Logi Receiver Consumer Control")));
        assert!(!scope.includes(Some("Pad")));
        // "Pad" is in both Pad groups, so only "Pad Mouse" identifies Pad 2.
        let (name, scope) = resolve_group("Pad 2", Some(&groups));
        assert_eq!(name, "Pad");
        assert!(scope.includes(Some("Pad Mouse")));
        assert!(!scope.includes(Some("Pad")));
        let (name, scope) = resolve_group("Pad 3", None);
        assert_eq!(name, "Pad 3");
        assert!(scope.includes(Some("Pad 3")));
    }
}
