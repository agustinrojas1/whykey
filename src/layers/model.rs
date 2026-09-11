use crate::key::KeyCombo;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Clone, Copy)]
pub(crate) struct ApplicationTarget<'a> {
    pub pid: u32,
    pub source: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalInput {
    pub bytes: Vec<u8>,
    pub description: String,
    pub confidence: InputConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum InputConfidence {
    Configured,
    Predicted,
    Observed,
}

impl TerminalInput {
    pub fn predicted(bytes: Vec<u8>, description: impl Into<String>) -> Self {
        Self {
            bytes,
            description: description.into(),
            confidence: InputConfidence::Predicted,
        }
    }

    pub fn configured(bytes: Vec<u8>, description: impl Into<String>) -> Self {
        Self {
            bytes,
            description: description.into(),
            confidence: InputConfidence::Configured,
        }
    }

    pub fn confidence_label(&self) -> &'static str {
        match self.confidence {
            InputConfidence::Configured => "configured",
            InputConfidence::Predicted => "predicted",
            InputConfidence::Observed => "observed",
        }
    }

    pub fn display_bytes(&self) -> String {
        format_bytes(&self.bytes)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerId {
    Remapper,
    Compositor,
    Terminal,
    Tty,
    Ime,
    Multiplexer,
    Application,
    Shell,
    Session,
    Diagnostic,
    Unknown,
}

/// Single internal route outcome. Serializers map it back to the existing
/// v1 (`status`) and (`propagation`) pair so automation keeps working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// No binding was found; the event keeps flowing.
    Pass,
    /// A binding was found and the event keeps flowing.
    HandledAndPassed,
    /// A binding was found and the event is consumed.
    Consumed,
    /// The event is redirected to another window.
    Redirected,
    /// A binding was found, but consumption cannot be proven.
    HandledUncertain,
    /// Handling state is unknown, but the event most likely keeps flowing.
    UncertainContinues,
    /// Handling state could not be determined.
    Unknown,
    /// The layer could not be inspected.
    Unavailable,
    /// Target process is active, but has no dedicated shortcut inspection adapter.
    UnadaptedTarget,
}

impl Outcome {
    pub fn from_parts(status: &LayerStatus, propagation: &Propagation) -> Self {
        match (status, propagation) {
            (LayerStatus::NotHandled, Propagation::Continues) => Self::Pass,
            (LayerStatus::Handled, Propagation::Continues) => Self::HandledAndPassed,
            (LayerStatus::Handled, Propagation::Stops) => Self::Consumed,
            (_, Propagation::Redirected) => Self::Redirected,
            (LayerStatus::Unavailable, _) => Self::Unavailable,
            (LayerStatus::Handled, Propagation::Indeterminate) => Self::HandledUncertain,
            (LayerStatus::Indeterminate, Propagation::Continues) => Self::UncertainContinues,
            _ => Self::Unknown,
        }
    }

    pub fn as_parts(self) -> (LayerStatus, Propagation) {
        match self {
            Self::Pass => (LayerStatus::NotHandled, Propagation::Continues),
            Self::HandledAndPassed => (LayerStatus::Handled, Propagation::Continues),
            Self::Consumed => (LayerStatus::Handled, Propagation::Stops),
            Self::Redirected => (LayerStatus::Handled, Propagation::Redirected),
            Self::HandledUncertain => (LayerStatus::Handled, Propagation::Indeterminate),
            Self::UncertainContinues => (LayerStatus::Indeterminate, Propagation::Continues),
            Self::Unknown | Self::UnadaptedTarget => {
                (LayerStatus::Indeterminate, Propagation::Indeterminate)
            }
            Self::Unavailable => (LayerStatus::Unavailable, Propagation::Indeterminate),
        }
    }
}

/// Explicit typed route order: remapper, compositor, terminal, IME, TTY,
/// multiplexer/session, application, shell.
pub const ROUTE_ORDER: &[LayerId] = &[
    LayerId::Remapper,
    LayerId::Compositor,
    LayerId::Terminal,
    LayerId::Ime,
    LayerId::Tty,
    LayerId::Multiplexer,
    LayerId::Session,
    LayerId::Application,
    LayerId::Shell,
];

/// One inspection request replacing positional arguments.
#[derive(Debug, Clone)]
pub struct InspectRequest<'a> {
    pub key: &'a KeyCombo,
    pub force_continue: bool,
    pub protocol_flags: Option<u32>,
    pub observed_bytes: Option<&'a [u8]>,
    pub physical_input: Option<PhysicalInput>,
    pub termios: Option<&'a libc::termios>,
    pub application_pid: Option<u32>,
    pub application_source: Option<&'a str>,
}

impl<'a> InspectRequest<'a> {
    pub fn new(key: &'a KeyCombo) -> Self {
        Self {
            key,
            force_continue: false,
            protocol_flags: None,
            observed_bytes: None,
            physical_input: None,
            termios: None,
            application_pid: None,
            application_source: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LayerStatus {
    NotHandled,
    Handled,
    Indeterminate,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Propagation {
    Continues,
    Stops,
    Redirected,
    Indeterminate,
}

/// One layer finding. Route state is typed: `id` names the emitting layer
/// and `outcome` is the single control value. The legacy `status` and
/// `propagation` pair exists only as derived serialization output for
/// schema v1/v2 compatibility.
///
/// Typed evidence for the binding a layer matched, when the adapter can name
/// it. Renderers must decide universal scope, action wording, and dispatcher
/// uncertainty from these fields, never by parsing human-readable details.
/// Only adapters with runtime binding data populate it; the rest leave
/// `LayerResult::binding` absent, and replaying an older report never
/// invents it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum UncertaintyReason {
    UnverifiedTerminalBytes,
    UnresolvedMode,
    OpaqueDispatcher,
    EndpointUnavailable,
    ModifierAmbiguity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BindingEvidence {
    pub dispatcher: Option<String>,
    pub action: Option<String>,
    pub description: Option<String>,
    pub submap: Option<String>,
    pub scope: BindingScope,
    pub source: Option<SourceLocation>,
    /// True when another matching binding is universal, even if this is the
    /// primary evidence selected for the layer.
    pub has_universal_match: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<UncertaintyReason>,
}

impl BindingEvidence {
    /// Dispatchers whose runtime effect Whykey cannot observe or execute:
    /// Lua/plugin hooks and colon-namespaced plugin calls.
    pub fn dispatcher_is_opaque(dispatcher: &str) -> bool {
        dispatcher == "__lua" || dispatcher.contains(':')
    }

    pub fn is_opaque(&self) -> bool {
        self.dispatcher
            .as_deref()
            .is_some_and(Self::dispatcher_is_opaque)
    }

    pub fn is_universal(&self) -> bool {
        self.scope == BindingScope::Universal
    }

    pub fn includes_universal_match(&self) -> bool {
        self.has_universal_match || self.is_universal()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum BindingScope {
    Universal,
    Submap(String),
    Unknown,
}

/// Static configuration hint for a runtime binding, when an adapter can
/// link the two without guessing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerResult {
    pub layer: &'static str,
    pub id: LayerId,
    pub outcome: Outcome,
    pub summary: String,
    pub details: Vec<String>,
    /// Diagnostic evidence shown only with `--verbose` in human output.
    /// JSON always combines both lists so machine output stays complete.
    pub verbose_details: Vec<String>,
    pub binding: Option<BindingEvidence>,
}

impl Serialize for LayerResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (status, propagation) = self.outcome.as_parts();
        let mut state = serializer.serialize_struct("LayerResult", 5)?;
        state.serialize_field("layer", &self.layer)?;
        state.serialize_field("status", &status)?;
        state.serialize_field("propagation", &propagation)?;
        state.serialize_field("summary", &self.summary)?;
        state.serialize_field("details", &self.all_details().collect::<Vec<_>>())?;
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalInput {
    pub device: Option<String>,
    pub keycode: crate::xkb::EvdevKeycode,
}

impl LayerResult {
    /// Constructs a Pass result (no matching binding found, continues).
    pub fn pass(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            layer,
            id,
            outcome: Outcome::Pass,
            summary: summary.into(),
            details,
            verbose_details: Vec::new(),
            binding: None,
        }
    }

    /// Constructs an Unavailable result (layer could not be inspected).
    pub fn unavailable(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            layer,
            id,
            outcome: Outcome::Unavailable,
            summary: summary.into(),
            details,
            verbose_details: Vec::new(),
            binding: None,
        }
    }

    /// Legacy schema v1 status, derived from the typed outcome.
    pub fn status(&self) -> LayerStatus {
        self.outcome.as_parts().0
    }
    /// Legacy schema v1 propagation, derived from the typed outcome.
    pub fn propagation(&self) -> Propagation {
        self.outcome.as_parts().1
    }
    pub fn continues(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::Pass | Outcome::HandledAndPassed | Outcome::UncertainContinues
        )
    }
    pub fn blocks_chain(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::Consumed | Outcome::Redirected | Outcome::Unavailable
        )
    }
    /// Every evidence line, normal then verbose. JSON renderers use this so
    /// machine-readable output stays complete while human output filters by
    /// verbosity through the field, never by searching detail text.
    pub fn all_details(&self) -> impl Iterator<Item = &String> {
        self.details.iter().chain(self.verbose_details.iter())
    }
}

pub fn format_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "ESC".to_owned(),
            0x20..=0x7e => char::from(*byte).to_string(),
            value => format!("0x{value:02x}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
