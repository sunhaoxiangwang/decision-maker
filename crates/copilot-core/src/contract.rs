//! `vexo-copilot/1` message shapes (PLAN v4 §9) and cue mapping.

use crate::limits::{LIST_ENTRY_MAX_BYTES, LIST_MAX_ENTRIES, REASON_MAX_BYTES, SNAPSHOT_MAX_BYTES};
use crate::text::bounded;
use serde::{Deserialize, Serialize};

pub const PROTOCOL: &str = "vexo-copilot/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Negotiation,
    Lunch,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Negotiation => "negotiation",
            Mode::Lunch => "lunch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Processing,
    Decided,
    AwaitingConfirmation,
    Invalidated,
    Unavailable,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    Proceed,
    Pause,
    Ask,
    Select,
    NoEligibleOption,
    None,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueKind {
    Decision,
    Option,
    Clear,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cue {
    pub kind: CueKind,
    pub name: String,
    pub rgb: String,
    pub pattern: String,
    pub period_ms: Option<u32>,
}

impl Cue {
    fn mk(kind: CueKind, name: &str, rgb: &str, pattern: &str, period: Option<u32>) -> Cue {
        Cue {
            kind,
            name: name.into(),
            rgb: rgb.into(),
            pattern: pattern.into(),
            period_ms: period,
        }
    }
    pub fn clear() -> Cue {
        Cue::mk(CueKind::Clear, "off", "#000000", "solid", None)
    }
    pub fn error() -> Cue {
        Cue::mk(CueKind::Error, "cyan", "#00FFFF", "blink", Some(150))
    }
    /// Cue for a standing decision. Proposals until Cy verifies them on the band.
    pub fn for_outcome(o: Outcome, option: Option<&str>) -> Cue {
        match (o, option) {
            (Outcome::Proceed, _) => Cue::mk(CueKind::Decision, "green", "#00FF00", "solid", None),
            (Outcome::Pause, _) => Cue::mk(CueKind::Decision, "red", "#FF0000", "blink", Some(400)),
            (Outcome::Ask, _) => {
                Cue::mk(CueKind::Decision, "amber", "#FFB000", "blink", Some(1000))
            }
            (Outcome::Select, Some("A")) => {
                Cue::mk(CueKind::Option, "green", "#00FF00", "solid", None)
            }
            (Outcome::Select, Some("B")) => {
                Cue::mk(CueKind::Option, "purple", "#8000FF", "solid", None)
            }
            (Outcome::Select, Some("C")) => {
                Cue::mk(CueKind::Option, "white", "#FFFFFF", "solid", None)
            }
            (Outcome::Unavailable, _) => Cue::error(),
            _ => Cue::clear(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>, retryable: bool) -> ApiError {
        let (m, _) = bounded(&message.into(), 512);
        ApiError {
            code: code.into(),
            message: m,
            retryable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub protocol: String,
    pub request_id: String,
    pub instance_id: String,
    pub kind: String,
    pub session_id: Option<String>,
    pub mode: Option<Mode>,
    pub rev: u64,
    pub state_rev: u64,
    pub generation: u64,
    pub error: Option<ApiError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub input_source: String,
    pub transcript_source: Option<String>,
    pub providers: String,
    pub pipeline: String,
    pub decision_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Timings {
    pub accept_to_publish: Option<u64>,
    pub extract: Option<u64>,
    pub evaluate: Option<u64>,
    pub decide: Option<u64>,
    /// Generation end-to-end: first acceptance to the latest publication (server clock).
    pub e2e: Option<u64>,
    /// Time spent waiting for the user to confirm (server clock).
    pub human_wait: Option<u64>,
    /// e2e minus human wait.
    pub processing: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CueSnapshot {
    #[serde(flatten)]
    pub header: Header,
    pub current: bool,
    pub phase: Phase,
    pub outcome: Outcome,
    pub selected_option: Option<String>,
    pub cue: Option<Cue>,
    pub actionable: bool,
    pub reason: Option<String>,
    pub missing: Vec<String>,
    pub conflicts: Vec<String>,
    pub unconfirmed: Vec<String>,
    pub conditions: Vec<String>,
    pub barriers: Vec<u64>,
    pub truncated: bool,
    pub provenance: Provenance,
    pub timings_ms: Timings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRef {
    pub source_id: String,
    pub client_turn_id: String,
    pub source_seq: u64,
    pub server_seq: Option<u64>,
    pub attempt: u32,
    pub attempts_total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TurnOutcome {
    Pending,
    Applied,
    AppliedSuperseded,
    FailedBeforeApply,
    FailedAfterApply,
    Cancelled,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnResult {
    #[serde(flatten)]
    pub header: Header,
    pub turn: TurnRef,
    pub turn_outcome: TurnOutcome,
    pub turn_effect: Option<String>,
    pub snapshot: Option<CueSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    #[serde(flatten)]
    pub header: Header,
    pub turn: Option<TurnRef>,
    pub accepted: bool,
    pub result: String,
    pub cue: Option<Cue>,
    pub actionable: bool,
    pub selected_option: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLatest {
    pub source_id: String,
    pub client_turn_id: String,
    pub server_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    #[serde(flatten)]
    pub header: Header,
    pub phase: Phase,
    pub latest_generation: u64,
    pub source_latest: Vec<SourceLatest>,
    pub cue: Option<Cue>,
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    #[serde(flatten)]
    pub header: Header,
    pub turn: Option<serde_json::Value>,
    pub cue: Option<Cue>,
    pub actionable: bool,
}

/// Bridge turn request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRequest {
    pub protocol: String,
    pub session_id: String,
    pub mode: Mode,
    pub source_id: String,
    pub client_turn_id: String,
    pub source_seq: u64,
    pub transcript: TranscriptIn,
    #[serde(default)]
    pub bridge_timing_ms: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptIn {
    pub text: String,
    pub transcript_source: String,
    #[serde(default)]
    pub speaker: Option<String>,
}

/// Bound reason/list sizes and the serialized snapshot (PLAN v4 §6).
pub fn bound_snapshot(s: &mut CueSnapshot) {
    if let Some(r) = s.reason.take() {
        s.reason = Some(bounded(&r, REASON_MAX_BYTES).0);
    }
    let mut truncated = false;
    for list in [
        &mut s.missing,
        &mut s.conflicts,
        &mut s.unconfirmed,
        &mut s.conditions,
    ] {
        if list.len() > LIST_MAX_ENTRIES {
            list.truncate(LIST_MAX_ENTRIES);
            truncated = true;
        }
        for e in list.iter_mut() {
            let (b, t) = bounded(e, LIST_ENTRY_MAX_BYTES);
            truncated |= t;
            *e = b;
        }
    }
    if s.barriers.len() > LIST_MAX_ENTRIES {
        s.barriers.truncate(LIST_MAX_ENTRIES);
        truncated = true;
    }
    let size = serde_json::to_vec(s).map(|v| v.len()).unwrap_or(usize::MAX);
    if size > SNAPSHOT_MAX_BYTES {
        let n = |l: &Vec<String>| format!("{} items", l.len());
        s.missing = vec![n(&s.missing)];
        s.conflicts = vec![n(&s.conflicts)];
        s.unconfirmed = vec![n(&s.unconfirmed)];
        s.conditions = vec![n(&s.conditions)];
        truncated = true;
    }
    s.truncated |= truncated;
}
