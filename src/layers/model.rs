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

/// Explicit typed route order, the default every other order derives from:
/// remapper, compositor, terminal, input method, TTY driver, multiplexer /
/// session, application, shell. [`route_order`] adjusts it for the two
/// placements that depend on the session; inspection, the report table and
/// the JSON/NDJSON path all take their order from there.
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

/// Where the input method meets a key relative to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImePlacement {
    /// A Wayland input-method-v2 frontend grabs the keyboard: the compositor
    /// hands each key to the input method before the focused terminal.
    BeforeTerminal,
    /// The terminal forwards keys to the input method (X11 XIM, GTK/Qt input
    /// method modules). Also the assumed order when the frontend cannot be
    /// determined.
    #[default]
    AfterTerminal,
}

/// The session facts that decide the route order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteContext {
    pub ime: ImePlacement,
    /// A multiplexer reads the terminal through its own client tty and
    /// writes what it forwards into the pane's pty, so inside one it comes
    /// before the pane's TTY driver.
    pub in_multiplexer: bool,
}

/// The single route order. Layers keep this order in the inspection result,
/// the report's route table and the JSON/NDJSON path.
pub fn route_order(context: RouteContext) -> Vec<LayerId> {
    let mut order: Vec<LayerId> = ROUTE_ORDER.to_vec();
    let position = |order: &[LayerId], id: LayerId| order.iter().position(|item| *item == id);
    if context.ime == ImePlacement::BeforeTerminal {
        order.retain(|id| *id != LayerId::Ime);
        let terminal = position(&order, LayerId::Terminal).expect("terminal is on the route");
        order.insert(terminal, LayerId::Ime);
    }
    if context.in_multiplexer {
        order.retain(|id| *id != LayerId::Tty);
        let multiplexer =
            position(&order, LayerId::Multiplexer).expect("multiplexer is on the route");
        // After the multiplexer and session stages, before the application.
        let session = position(&order, LayerId::Session).expect("session is on the route");
        order.insert(multiplexer.max(session) + 1, LayerId::Tty);
    }
    order
}

/// The route position of a layer id: the multiplexer and session stages
/// share one position in the report.
pub fn route_stage(id: LayerId) -> LayerId {
    match id {
        LayerId::Session => LayerId::Multiplexer,
        other => other,
    }
}

/// Recovers the session facts from layers already emitted in route order
/// (a report has no session to ask). The input method records its own
/// placement; the multiplexer order is read from the emitted order, and a
/// route that stopped before showing it keeps the default, which only
/// reorders rows nothing answered for.
pub fn route_context_of(layers: &[LayerResult]) -> RouteContext {
    let index = |wanted: &dyn Fn(LayerId) -> bool| layers.iter().position(|l| wanted(l.id));
    let terminal = index(&|id| id == LayerId::Terminal);
    let ime = index(&|id| id == LayerId::Ime);
    let tty = index(&|id| id == LayerId::Tty);
    let session = index(&|id| matches!(id, LayerId::Session | LayerId::Multiplexer));
    let recorded = layers.iter().find_map(|layer| layer.ime_placement);
    RouteContext {
        ime: match (recorded, ime, terminal) {
            (Some(placement), _, _) => placement,
            (None, Some(ime), Some(terminal)) if ime < terminal => ImePlacement::BeforeTerminal,
            _ => ImePlacement::AfterTerminal,
        },
        in_multiplexer: matches!((session, tty), (Some(session), Some(tty)) if session < tty),
    }
}

/// Earlier steps of a multi-key sequence, as delivered to the stateful
/// layers that interpret sequences (tmux key tables, the shell editor).
/// Steps a layer never received (consumed upstream) are not recorded for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SequencePrefix {
    /// Earlier keys that reached the multiplexer layer, oldest first.
    pub multiplexer_keys: Vec<KeyCombo>,
    /// Terminal input of earlier keys that reached the shell, oldest first.
    pub shell_inputs: Vec<TerminalInput>,
    /// An earlier key reached the shell without predictable bytes, so the
    /// shell editor's sequence state is unknown.
    pub shell_input_unknown: bool,
}

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
    /// Earlier steps when `key` is not the first key of a sequence.
    pub sequence_prefix: Option<&'a SequencePrefix>,
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
            sequence_prefix: None,
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
    /// The binding comes from built-in defaults or static startup files, not
    /// from the live shell; the running shell may have rebound the key.
    UnverifiedDefault,
    /// The adapter could not read the keys of `candidates` bindings with the
    /// requested modifiers, so any of them may use this key.
    /// `consumes_if_matched` is true when every candidate stops the key.
    UnresolvedBindingKey {
        candidates: usize,
        consumes_if_matched: bool,
    },
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
    /// Construct evidence for a named action with the common optional fields
    /// unset. Adapters can add description, submap, device, and uncertainty
    /// explicitly instead of assembling a partially meaningful struct.
    pub fn action(
        action: impl Into<String>,
        scope: BindingScope,
        source: Option<SourceLocation>,
    ) -> Self {
        Self {
            dispatcher: None,
            action: Some(action.into()),
            description: None,
            submap: None,
            scope,
            source,
            has_universal_match: false,
            uncertainty: None,
        }
    }

    pub fn with_dispatcher(mut self, dispatcher: impl Into<String>) -> Self {
        self.dispatcher = Some(dispatcher.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_submap(mut self, submap: impl Into<String>) -> Self {
        self.submap = Some(submap.into());
        self
    }

    pub fn with_uncertainty(mut self, reason: UncertaintyReason) -> Self {
        self.uncertainty = Some(reason);
        self
    }

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

impl SourceLocation {
    pub fn new(file: impl Into<String>, line: Option<u32>) -> Self {
        Self {
            file: file.into(),
            line,
        }
    }
}

/// One binding returned by a supported desktop source: the single inventory
/// representation every adapter produces. `context`, `device`, and `submap`
/// are typed payload metadata, never inferred; `None` means the adapter did
/// not report one, which never matches an active `--device`/`--submap` filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BindingRecord {
    pub source: String,
    pub key: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submap: Option<String>,
    pub certainty: String,
}

impl BindingRecord {
    /// Ordinary constructor with sensible defaults: no context, device, or
    /// submap metadata. Use [`Self::with_context`] for the common context
    /// case; the Hyprland adapter fills device/submap directly.
    pub fn new(
        source: impl Into<String>,
        key: String,
        action: String,
        certainty: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            key,
            action,
            context: None,
            device: None,
            submap: None,
            certainty: certainty.into(),
        }
    }

    /// Attach the configuration context the binding was read from.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }
}

/// How directly a layer's answer was established, strongest first. Adapters
/// set it on the results they produce ([`LayerResult::with_tier`]); reports
/// never infer it from evidence text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    /// A capture saw the key pass (or stop at) this layer.
    Observed,
    /// The running program answered: compositor IPC, gsettings, kglobalaccel,
    /// a shell runtime dump, tmux, or the X server's grab state.
    LiveQuery,
    /// Parsed from configuration files; the running program may differ.
    ConfigFile,
    /// Built-in defaults; the running program may differ.
    Default,
}

impl EvidenceTier {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::LiveQuery => "live query",
            Self::ConfigFile => "config file (static)",
            Self::Default => "default (unverified)",
        }
    }
}

/// What a [`Outcome::Pass`] rests on: the tier of the answer and the
/// sources that were consulted. "Found nothing" is a claim about the places
/// that were looked at, so a pass cannot exist without naming them; a layer
/// that did not look at everything that can take the key reports
/// [`Outcome::UncertainContinues`] instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checked {
    pub tier: EvidenceTier,
    pub sources: Vec<String>,
}

impl Checked {
    pub fn new<S: Into<String>>(tier: EvidenceTier, sources: impl IntoIterator<Item = S>) -> Self {
        Self {
            tier,
            sources: sources.into_iter().map(Into::into).collect(),
        }
    }

    /// Evidence for fixtures that only need some pass to exist.
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self::new(EvidenceTier::LiveQuery, ["test fixture"])
    }

    /// The pass rests on at least one named source.
    pub fn is_complete(&self) -> bool {
        self.sources.iter().any(|source| !source.trim().is_empty())
    }
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
    /// Dead keysym (e.g. `dead_acute`) whose composition this layer starts:
    /// the key is held for composition, not bound, and no bytes are sent
    /// until the next key completes it.
    pub dead_key: Option<String>,
    /// Where this answer came from; `None` when nothing answered for the
    /// layer (not running, no data read).
    pub tier: Option<EvidenceTier>,
    /// What a pass rests on. Present on every [`Outcome::Pass`] result and
    /// absent on all others (see [`LayerResult::pass`]).
    pub checked: Option<Checked>,
    /// The layer is the key's last consumer (a shell's line editor): even
    /// an unbound key is read and dropped or inserted there, so nothing
    /// downstream receives it.
    pub endpoint: bool,
    /// Other matching bindings that fire together with `binding` (Hyprland
    /// runs every matching non-submap bind), in execution order.
    pub also_fires: Vec<BindingEvidence>,
    /// Where the input method sat on this route; set only on the input
    /// method's own result. The report needs it even when the route stopped
    /// at the input method and no terminal result exists to compare with.
    pub ime_placement: Option<ImePlacement>,
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
    /// Ordinary constructor with sensible defaults: no verbose-only details,
    /// no binding evidence. Use [`Self::with_binding`] and
    /// [`Self::with_verbose_details`] for the non-default cases.
    pub fn new(
        layer: &'static str,
        id: LayerId,
        outcome: Outcome,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self::raw(layer, id, outcome, summary, details).without_unevidenced_pass()
    }

    fn raw(
        layer: &'static str,
        id: LayerId,
        outcome: Outcome,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self {
            layer,
            id,
            outcome,
            summary: summary.into(),
            details,
            verbose_details: Vec::new(),
            binding: None,
            dead_key: None,
            tier: None,
            checked: None,
            endpoint: false,
            also_fires: Vec::new(),
            ime_placement: None,
        }
    }

    /// A pass that carries no evidence cannot be reported. Builders that
    /// reach this with [`Outcome::Pass`] are a programming error (debug
    /// builds and tests fail); a release build reports the honest
    /// alternative: the key most likely continues, but nothing says what was
    /// checked.
    fn without_unevidenced_pass(mut self) -> Self {
        let evidenced = self.checked.as_ref().is_some_and(Checked::is_complete);
        if self.outcome == Outcome::Pass && !evidenced {
            debug_assert!(
                false,
                "{}: a Pass needs Checked evidence (use LayerResult::pass or ::decided)",
                self.layer
            );
            self.outcome = Outcome::UncertainContinues;
            self.checked = None;
            self.details
                .push("whykey recorded no evidence for this pass, so it is not reported".into());
        }
        self
    }

    /// What a pass rests on; `None` for every other outcome.
    pub fn pass_evidence(&self) -> Option<&Checked> {
        self.checked
            .as_ref()
            .filter(|_| self.outcome == Outcome::Pass)
    }

    /// The same guard the constructors apply, for a result whose fields were
    /// changed after construction.
    pub fn with_pass_evidence_enforced(self) -> Self {
        self.without_unevidenced_pass()
    }

    /// True when this result is a pass without recorded evidence. Routes are
    /// validated with it before they are reported.
    pub fn is_unevidenced_pass(&self) -> bool {
        self.outcome == Outcome::Pass && !self.checked.as_ref().is_some_and(Checked::is_complete)
    }

    /// Record where the input method met the key on this route.
    pub fn with_ime_placement(mut self, placement: ImePlacement) -> Self {
        self.ime_placement = Some(placement);
        self
    }

    /// Record where this answer came from.
    pub fn with_tier(mut self, tier: EvidenceTier) -> Self {
        self.tier = Some(tier);
        if let Some(checked) = &mut self.checked {
            checked.tier = tier;
        }
        self
    }

    /// Record where this answer came from when known.
    pub fn with_optional_tier(mut self, tier: Option<EvidenceTier>) -> Self {
        match (&mut self.checked, tier) {
            // A pass keeps the tier it was checked at unless a stronger
            // statement replaces it; it never loses it.
            (Some(_), None) => {}
            (Some(checked), Some(tier)) => {
                checked.tier = tier;
                self.tier = Some(tier);
            }
            (None, tier) => self.tier = tier,
        }
        self
    }

    /// Mark this layer as the key's last consumer.
    pub fn as_endpoint(mut self) -> Self {
        self.endpoint = true;
        self
    }

    /// True when nothing downstream of this layer receives the key.
    pub fn is_endpoint(&self) -> bool {
        self.endpoint
    }

    /// Attach typed binding evidence to the result.
    pub fn with_binding(mut self, binding: BindingEvidence) -> Self {
        self.binding = Some(binding);
        self
    }

    /// Attach the other matching bindings that fire together with
    /// [`Self::binding`], in the order the program runs them.
    pub fn with_also_fires(mut self, also_fires: Vec<BindingEvidence>) -> Self {
        self.also_fires = also_fires;
        self
    }

    /// Attach verbose-only evidence lines (shown with `--verbose` in text).
    pub fn with_verbose_details(mut self, verbose_details: Vec<String>) -> Self {
        self.verbose_details = verbose_details;
        self
    }

    /// A pass: nothing in `checked.sources` takes the key. The tier and the
    /// sources are required and recorded on the result.
    pub fn pass(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
        checked: Checked,
    ) -> Self {
        let mut result = Self::raw(layer, id, Outcome::Pass, summary, details);
        result.tier = Some(checked.tier);
        result.checked = Some(checked);
        result.without_unevidenced_pass()
    }

    /// Promote a provisional, still-uncertain result to a pass once every
    /// source that can take the key has been checked.
    pub fn confirmed_pass(mut self, checked: Checked) -> Self {
        debug_assert_eq!(self.outcome, Outcome::UncertainContinues);
        self.outcome = Outcome::Pass;
        self.tier = Some(checked.tier);
        self.checked = Some(checked);
        self.without_unevidenced_pass()
    }

    /// A result whose outcome the adapter computes: `checked` is what the
    /// adapter looked at, recorded only when the outcome is a pass.
    pub fn decided(
        layer: &'static str,
        id: LayerId,
        outcome: Outcome,
        checked: Checked,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        if outcome == Outcome::Pass {
            Self::pass(layer, id, summary, details, checked)
        } else {
            Self::new(layer, id, outcome, summary, details)
        }
    }

    pub fn handled_and_passed(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self::new(layer, id, Outcome::HandledAndPassed, summary, details)
    }

    pub fn consumed(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self::new(layer, id, Outcome::Consumed, summary, details)
    }

    pub fn uncertain_continues(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self::new(layer, id, Outcome::UncertainContinues, summary, details)
    }

    /// Constructs an Unavailable result (layer could not be inspected).
    pub fn unavailable(
        layer: &'static str,
        id: LayerId,
        summary: impl Into<String>,
        details: Vec<String>,
    ) -> Self {
        Self::new(layer, id, Outcome::Unavailable, summary, details)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_outcome_constructors_keep_legacy_parts_compatible() {
        let passed = LayerResult::handled_and_passed(
            "fixture",
            LayerId::Compositor,
            "handled but forwarded",
            vec![],
        );
        assert_eq!(passed.outcome, Outcome::HandledAndPassed);
        assert_eq!(passed.status(), LayerStatus::Handled);
        assert_eq!(passed.propagation(), Propagation::Continues);

        let consumed = LayerResult::consumed("fixture", LayerId::Compositor, "consumed", vec![]);
        assert_eq!(consumed.outcome, Outcome::Consumed);
        assert!(consumed.blocks_chain());
    }

    #[test]
    fn a_pass_records_the_tier_and_the_sources_it_checked() {
        let pass = LayerResult::pass(
            "fixture",
            LayerId::Compositor,
            "no binding",
            vec![],
            Checked::new(EvidenceTier::ConfigFile, ["rc.xml", "defaults"]),
        );
        assert_eq!(pass.outcome, Outcome::Pass);
        assert_eq!(pass.tier, Some(EvidenceTier::ConfigFile));
        let checked = pass.pass_evidence().expect("a pass carries its evidence");
        assert_eq!(checked.sources, ["rc.xml", "defaults"]);
        assert!(!pass.is_unevidenced_pass());
        // Only a pass names what it checked.
        let consumed = LayerResult::consumed("fixture", LayerId::Compositor, "bound", vec![]);
        assert!(consumed.pass_evidence().is_none());
    }

    /// `LayerResult::new` is for every outcome except a pass: a pass built
    /// without evidence is a bug that debug builds and tests refuse.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a Pass needs Checked evidence")]
    fn a_pass_without_evidence_cannot_be_built() {
        let _ = LayerResult::new(
            "fixture",
            LayerId::Compositor,
            Outcome::Pass,
            "none",
            vec![],
        );
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a Pass needs Checked evidence")]
    fn a_pass_that_names_no_source_cannot_be_built() {
        let _ = LayerResult::pass(
            "fixture",
            LayerId::Compositor,
            "none",
            vec![],
            Checked::new(EvidenceTier::LiveQuery, Vec::<String>::new()),
        );
    }

    /// Without debug assertions the same mistake is reported as the honest
    /// alternative, never as a pass.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_pass_without_evidence_is_reported_as_uncertain() {
        let result = LayerResult::new(
            "fixture",
            LayerId::Compositor,
            Outcome::Pass,
            "none",
            vec![],
        );
        assert_eq!(result.outcome, Outcome::UncertainContinues);
        assert!(!result.is_unevidenced_pass());
    }

    #[test]
    fn every_pass_on_a_real_route_names_what_it_checked() {
        for key in [
            "ctrl+alt+p",
            "escape",
            "ctrl+f9",
            "super+return",
            "alt+f",
            "a",
        ] {
            let key: crate::key::KeyCombo = key.parse().unwrap();
            for layer in crate::layers::inspect_default_chain(&key) {
                assert!(
                    !layer.is_unevidenced_pass(),
                    "{} reports a pass without evidence",
                    layer.layer
                );
                if layer.outcome == Outcome::Pass {
                    let checked = layer.pass_evidence().unwrap();
                    assert_eq!(layer.tier, Some(checked.tier), "{}", layer.layer);
                    assert!(!checked.sources.is_empty(), "{}", layer.layer);
                } else {
                    assert!(layer.pass_evidence().is_none(), "{}", layer.layer);
                }
            }
        }
    }

    #[test]
    fn binding_evidence_builder_is_explicit_about_optional_claims() {
        let evidence = BindingEvidence::action(
            "exec foot",
            BindingScope::Submap("default".into()),
            Some(SourceLocation::new("fixture", Some(7))),
        )
        .with_dispatcher("exec")
        .with_description("launch terminal")
        .with_uncertainty(UncertaintyReason::EndpointUnavailable);
        assert_eq!(evidence.action.as_deref(), Some("exec foot"));
        assert_eq!(evidence.dispatcher.as_deref(), Some("exec"));
        assert_eq!(evidence.source.as_ref().unwrap().line, Some(7));
        assert_eq!(
            evidence.uncertainty,
            Some(UncertaintyReason::EndpointUnavailable)
        );
    }
}
