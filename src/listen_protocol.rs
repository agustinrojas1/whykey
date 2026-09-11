//! Terminal protocol boundary for [`crate::listen`].
//!
//! The listener owns terminal lifecycle and restoration; this module owns the
//! small, pure protocol surface used by that lifecycle. Keeping the boundary
//! explicit lets the evdev and terminal transports evolve independently while
//! preserving the existing Kitty/legacy decoder behavior.

use std::io;

use crate::listen::ObservedKey;

/// Decode one terminal byte sequence without changing terminal state.
pub(crate) fn decode(bytes: Vec<u8>) -> Result<ObservedKey, io::Error> {
    crate::listen::decode_bytes(bytes)
}

/// Parse a Kitty keyboard-protocol response (`CSI ? flags u`).
pub(crate) fn parse_protocol_response(response: &[u8]) -> Option<u32> {
    crate::listen::parse_protocol_response(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_boundary_decodes_legacy_control_bytes() {
        let event = decode(vec![0x1a]).expect("ctrl-z should decode");
        assert_eq!(event.combo.to_string(), "CTRL + Z");
    }

    #[test]
    fn protocol_boundary_parses_only_complete_kitty_responses() {
        assert_eq!(parse_protocol_response(b"\x1b[?9u"), Some(9));
        assert_eq!(parse_protocol_response(b"\x1b[1;5D"), None);
    }
}
