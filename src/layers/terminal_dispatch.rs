//! One terminal path: route evidence plus normal PTY bytes.
//!
//! Ghostty and the generic terminal adapter share one dispatcher with
//! Ghostty, Kitty, Alacritty, Foot, WezTerm, Konsole, and generic variants.
//! Configuration is resolved once per analysis and reused for both the
//! visible terminal layer and downstream TTY/shell byte prediction.

use crate::key::KeyCombo;
use crate::layers::{LayerResult, TerminalInput, ghostty, terminal_app};

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
    pub fn analyze(key: &KeyCombo, protocol_flags: Option<u32>) -> Self {
        if let Some(name) = terminal_app::detected_name() {
            let (layer, input) = terminal_app::analyze(key);
            return Self {
                layer,
                input,
                variant: name,
            };
        }
        let terminal = ghostty::Ghostty::default();
        let (layer, input) = terminal.analyze(key, protocol_flags);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatcher_names_the_active_terminal_variant() {
        let key: KeyCombo = "ctrl+f".parse().unwrap();
        let analysis = TerminalAnalysis::analyze(&key, None);
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
