//! One terminal path: route evidence plus normal PTY bytes.
//!
//! Ghostty and the generic terminal adapter share one dispatcher with
//! Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, and generic variants.
//! Configuration is resolved once per analysis and reused for both the
//! visible terminal layer and downstream TTY/shell byte prediction.

use crate::environment::Environment;
use crate::layers::{
    InspectRequest, LayerResult, PhysicalInput, TerminalInput, ghostty, hyprland, terminal_app,
};
use crate::xkb::{self, KeyboardLayout};

/// Mod5: AltGr, the only modifier whose text depends on the keymap.
const ALTGR_MASK: u32 = 128;

/// Terminal finding plus the bytes the terminal would normally emit.
#[derive(Debug, Clone)]
pub struct TerminalAnalysis {
    pub layer: LayerResult,
    pub input: Option<TerminalInput>,
    pub variant: &'static str,
}

impl TerminalAnalysis {
    /// Detect the active terminal once, inspect once, resolve normal input
    /// bytes from the same loaded configuration.
    pub fn analyze(request: &InspectRequest<'_>, session: &Environment) -> Self {
        let key = request.key;
        // Only AltGr text needs the keymap; skip compiling it otherwise.
        let layout = (key.modmask() & ALTGR_MASK != 0)
            .then(|| active_layout(session, request.physical_input.as_ref()))
            .flatten();
        let layout = layout.as_ref();
        if let Some(name) = terminal_app::detected_name() {
            let (layer, input) = terminal_app::analyze(key, request.protocol_flags, layout);
            return Self {
                layer,
                input,
                variant: name,
            };
        }
        let terminal = ghostty::Ghostty::default();
        let (layer, input) = terminal.analyze(key, request.protocol_flags, layout);
        let variant = if ghostty::is_active() {
            "Ghostty"
        } else {
            "Terminal"
        };
        Self {
            layer,
            input,
            variant,
        }
    }

    /// Terminal identity named by both text reports and schema context.
    pub fn identity() -> Option<&'static str> {
        crate::layers::terminal_identity()
    }
}

/// The main keyboard's compiled keymap: the compositor's when it reports
/// one, otherwise an explicit XKB layout from the environment.
fn active_layout(
    session: &Environment,
    physical: Option<&PhysicalInput>,
) -> Option<KeyboardLayout> {
    let physical_keycode = physical.and_then(|input| input.keycode.to_xkb());
    if let Some(devices) = session
        .hyprland_probe
        .as_ref()
        .and_then(|probe| probe.devices_json())
    {
        return Some(KeyboardLayout {
            keymap: hyprland::compile_main_xkb_keymap(devices)?,
            group: hyprland::main_keyboard_active_layout_index(devices)?,
            physical_keycode,
        });
    }
    Some(KeyboardLayout {
        keymap: xkb::compile_keymap_from_environment()?,
        group: xkb::group_from_environment(),
        physical_keycode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::KeyCombo;

    #[test]
    fn dispatcher_names_the_active_terminal_variant() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let session = Environment::collect();
        let analysis = TerminalAnalysis::analyze(&InspectRequest::new(&key), &session);
        assert_eq!(analysis.layer.id, crate::layers::LayerId::Terminal);
        assert_eq!(
            TerminalAnalysis::identity().is_some(),
            analysis.variant != "Terminal"
        );
    }

    #[test]
    fn single_terminal_layer_per_inspection() {
        let key: KeyCombo = "ctrl+z".parse().unwrap();
        let layers = crate::layers::inspect_default_chain(&key);
        let terminal_layers = layers
            .iter()
            .filter(|layer| layer.id == crate::layers::LayerId::Terminal)
            .count();
        assert!(
            terminal_layers <= 1,
            "one selected terminal contributes one terminal layer, found {terminal_layers}"
        );
    }
}
