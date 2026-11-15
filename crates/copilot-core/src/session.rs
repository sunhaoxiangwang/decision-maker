//! Pure session state machine: turns, offers, facts, conditions, barriers,
//! two-phase application and decision publication (PLAN v3 §3-4, v4 §2-4).
//! The server wraps a `Session` in a mutex and performs provider I/O outside it.

use crate::contextual::NoulVerdict;
use crate::contract::*;
use crate::limits::*;
use crate::lunch::{self, LunchState};
use crate::money::fmt_cents;
use crate::negotiation::*;
use crate::text::{bounded, excerpt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    pub id: u64,
    pub version: u64,
    pub stmt: Stmt,
    pub confirmed: bool,
    pub source_seq: Option<u64>,
    pub excerpt: String,
    pub excerpt_truncated: bool,
    pub byte_range: Option<(usize, usize)>,
    pub extractor: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionKind {
    ContextualConcern,
    ContextualUncertain,
    ContextLimit,
    /// More conditions than the per-offer limit: one blocking record spanning them.
    Overflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStatus {
    Unresolved,
    ResolvedByUser,
    SupersededByOffer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub id: u64,
    pub version: u64,
    pub kind: ConditionKind,
    pub covers: (u64, u64),
    pub status: ConditionStatus,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub version: u64,
    pub facts: Vec<Fact>,
    pub conditions: Vec<Condition>,
    pub created_seq: u64,
    pub turn_seqs: Vec<u64>,
    pub resolved_through_seq: u64,
    /// Highest turn number dropped from the bounded `turn_seqs` list.
    pub dropped_through: u64,
    /// Rejected critical interpretations; open ones block positive outcomes.
    pub unresolved: Vec<Unresolved>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unresolved {
    pub id: u64,
    pub version: u64,
    pub field: String,
    pub reason: String,
    pub excerpt: String,
    pub source_seq: u64,
    pub open: bool,
    /// Rejected readings merged into this record after the per-offer limit.
    pub merged: u32,
}

impl Offer {
    pub fn stmts(&self) -> Vec<&Stmt> {
        self.facts.iter().map(|f| &f.stmt).collect()
    }
    pub fn open_unresolved(&self) -> Vec<&Unresolved> {
        self.unresolved.iter().filter(|u| u.open).collect()
    }
    /// Record a rejected interpretation without silently dropping evidence:
    /// dismissed records are reclaimed first; past the limit, readings merge into
    /// one blocking overflow record that keeps the latest excerpt and turn range.
    fn record_unresolved(&mut self, u: Unresolved) {
        if self.unresolved.len() >= UNRESOLVED_PER_OFFER {
            self.unresolved.retain(|x| x.open);
        }
        if let Some(ov) = self
            .unresolved
            .iter_mut()
            .find(|x| x.open && x.field == "overflow")
        {
            ov.merged += 1;
            ov.version += 1;
            ov.reason = format!(
                "{} rejected readings beyond the {UNRESOLVED_PER_OFFER}-record limit (turns {}-{}); latest: {}",
                ov.merged, ov.source_seq, u.source_seq, u.reason
            );
            ov.excerpt = u.excerpt;
            return;
        }
        if self.unresolved.len() + 1 >= UNRESOLVED_PER_OFFER {
            self.unresolved.push(Unresolved {
                field: "overflow".into(),
                merged: 1,
                reason: format!(
                    "rejected readings reached the {UNRESOLVED_PER_OFFER}-record limit (from turn {}); latest: {}",
                    u.source_seq, u.reason
                ),
                ..u
            });
            return;
        }
        self.unresolved.push(u);
    }

    /// Record a condition: resolved/superseded records are reclaimed first; if the
    /// offer is still full, one blocking overflow condition spans every later
    /// observation. Returns true when state changed.
    fn record_condition(&mut self, c: Condition) -> bool {
        if self.conditions.len() >= CONDITIONS_PER_OFFER {
            self.conditions
                .retain(|x| x.status == ConditionStatus::Unresolved);
        }
        if let Some(ov) = self
            .conditions
            .iter_mut()
            .find(|x| x.status == ConditionStatus::Unresolved && x.kind == ConditionKind::Overflow)
        {
            ov.covers = (ov.covers.0.min(c.covers.0), ov.covers.1.max(c.covers.1));
            ov.version += 1;
            ov.note = format!(
                "more conditions than recordable (turns {}-{}); latest: {}",
                ov.covers.0, ov.covers.1, c.note
            );
            return true;
        }
        if self.conditions.len() + 1 >= CONDITIONS_PER_OFFER {
            let covers = self
                .conditions
                .iter()
                .filter(|x| x.status == ConditionStatus::Unresolved)
                .fold(c.covers, |a, x| (a.0.min(x.covers.0), a.1.max(x.covers.1)));
            self.conditions.push(Condition {
                kind: ConditionKind::Overflow,
                covers,
                note: format!(
                    "condition limit reached (turns {}-{}); latest: {}",
                    covers.0, covers.1, c.note
                ),
                ..c
            });
            return true;
        }
        self.conditions.push(c);
        true
    }

    fn push_turn(&mut self, seq: u64) {
        self.turn_seqs.push(seq);
        while self.turn_seqs.len() > CONTEXT_MAX_TURNS + 1 {
            let d = self.turn_seqs.remove(0);
            self.dropped_through = self.dropped_through.max(d);
        }
    }
    pub fn unresolved_conditions(&self) -> Vec<&Condition> {
        self.conditions
            .iter()
            .filter(|c| c.status == ConditionStatus::Unresolved)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    pub id: String,
    pub created_seq: u64,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnStatus {
    Queued,
    Extracting,
    Applied,
    Evaluating,
    Completed { published: bool },
    FailedBeforeApply,
    FailedAfterApply,
    Cancelled,
    Discarded,
}

impl TurnStatus {
    pub fn is_pre_apply_pending(self) -> bool {
        matches!(self, TurnStatus::Queued | TurnStatus::Extracting)
    }
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TurnStatus::Completed { .. }
                | TurnStatus::FailedBeforeApply
                | TurnStatus::FailedAfterApply
                | TurnStatus::Cancelled
                | TurnStatus::Discarded
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub server_seq: u64,
    pub source_id: String,
    pub client_turn_id: String,
    pub source_seq: u64,
    pub mode: Mode,
    pub text: String,
    pub transcript_source: String,
    pub speaker: Option<String>,
    pub input_source: String,
    pub status: TurnStatus,
    pub attempt: u32,
    pub attempts_total: u32,
    pub effect_applied: bool,
    pub effect: Option<String>,
    pub error: Option<ApiError>,
    pub rejected_terms: Vec<String>,
    pub accepted_ms: u64,
    pub timings: Timings,
    pub awaiting_since_ms: Option<u64>,
    pub human_wait_ms: u64,
    /// Validated terms of an unclear turn, kept for explicit user resolution.
    pub pending_terms: Vec<(Stmt, String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub outcome: Outcome,
    pub awaiting_confirmation: bool,
    pub selected_option: Option<String>,
    pub reason: String,
    pub missing: Vec<String>,
    pub conflicts: Vec<String>,
    pub unconfirmed: Vec<String>,
    pub conditions: Vec<String>,
    pub decision_source: String,
    pub error: Option<ApiError>,
}

impl Decision {
    pub fn simple(outcome: Outcome, reason: impl Into<String>, source: &str) -> Decision {
        Decision {
            outcome,
            awaiting_confirmation: false,
            selected_option: None,
            reason: reason.into(),
            missing: vec![],
            conflicts: vec![],
            unconfirmed: vec![],
            conditions: vec![],
            decision_source: source.into(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Job {
    Turn {
        seq: u64,
    },
    Evaluate {
        trigger: Option<u64>,
    },
    /// Voice-context extraction (negotiation only).
    Context {
        seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractCtx {
    pub seq: u64,
    pub attempt: u32,
    pub mode: Mode,
    pub text: String,
    pub active_offer_id: Option<String>,
    pub active_terms: String,
    pub slot_versions: BTreeMap<Slot, Vec<(u64, u64)>>,
    pub lunch_option_versions: [u64; 3],
    pub lunch_prompt: String,
    pub reextracted: bool,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PhaseA {
    Applied,
    /// Versions changed between snapshot and apply: extract again from a fresh snapshot.
    Reextract,
    /// Turn cancelled or session closed: nothing applied.
    Dropped,
    /// The job deadline passed: nothing applied.
    Expired,
    /// The extraction violates the server-side contract (e.g. too many terms).
    Invalid(ApiError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextualRequest {
    Noul {
        terms: String,
        window: String,
    },
    Choice {
        soft_prefs: String,
        options: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvalCtx {
    pub trigger: Option<u64>,
    pub state_rev: u64,
    pub memory_version: u64,
    pub offer_id: Option<String>,
    pub covers: (u64, u64),
    pub request: ContextualRequest,
    pub base: Decision,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EvalStart {
    /// A later turn is pending; this turn's decision is superseded.
    Superseded,
    /// Decision committed without a contextual call.
    Committed,
    /// Nothing to evaluate (pending turn or no goals).
    Skipped,
    NeedsContextual(Box<EvalCtx>),
    /// The job deadline passed before publication.
    Expired,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContextualResult {
    Noul(Result<NoulVerdict, ApiError>),
    Choice(Result<crate::contextual::ChoiceAnswer, ApiError>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Commit {
    Committed,
    Stale,
    Superseded,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accepted {
    New(u64),
    Duplicate(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptReq {
    pub mode: Mode,
    pub source_id: String,
    pub client_turn_id: String,
    pub source_seq: Option<u64>,
    pub text: String,
    pub transcript_source: String,
    pub speaker: Option<String>,
    pub input_source: String,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub mode: Mode,
    pub instance_id: String,
    pub providers: String,
    pub rev: u64,
    pub state_rev: u64,
    pub generation: u64,
    pub closed: bool,
    pub phase: Phase,
    pub decision: Option<Decision>,
    pub goals: NegGoals,
    pub goals_confirmed: bool,
    pub goals_version: u64,
    pub active: Option<Offer>,
    pub archived: VecDeque<Offer>,
    pub tombstones: VecDeque<Tombstone>,
    pub next_offer_no: u64,
    pub next_fact_id: u64,
    pub next_cond_id: u64,
    pub turns: VecDeque<TurnRecord>,
    pub transcript_bytes: usize,
    pub next_server_seq: u64,
    pub source_last_seq: BTreeMap<String, u64>,
    pub source_latest: BTreeMap<String, (String, u64)>,
    pub barriers: BTreeSet<u64>,
    pub ambiguous: BTreeSet<u64>,
    pub queue: VecDeque<Job>,
    pub lunch: Option<LunchState>,
    pub last_publish_ms: Option<u64>,
    /// Latest memory version this session was notified of.
    pub memory_seen: u64,
    /// A user-triggered evaluation failed; retryable without changing facts.
    pub eval_failed: Option<ApiError>,
    /// Latest voice-context update (negotiation only) and bounded history.
    pub context: Option<crate::context::ContextUpdate>,
    pub context_history: VecDeque<crate::context::ContextUpdate>,
    pub context_proposals: u64,
    /// COPILOT_VOICE_GOALS=auto: wave updates apply without confirmation, and the first
    /// updates set the goals (decisions start once all four have been stated).
    pub auto_goals: bool,
    /// Goal fields stated by voice while goals were still unset (auto mode).
    pub stated_goals: Vec<crate::context::CtxField>,
}

fn stale(msg: &str) -> ApiError {
    ApiError::new("stale_form", msg, false)
}

impl Session {
    pub fn new(id: &str, mode: Mode, instance_id: &str, providers: &str) -> Session {
        Session {
            id: id.into(),
            mode,
            instance_id: instance_id.into(),
            providers: providers.into(),
            rev: 1,
            state_rev: 1,
            generation: 0,
            closed: false,
            phase: Phase::Idle,
            decision: None,
            goals: NegGoals::default(),
            goals_confirmed: false,
            goals_version: 1,
            active: None,
            archived: VecDeque::new(),
            tombstones: VecDeque::new(),
            next_offer_no: 1,
            next_fact_id: 1,
            next_cond_id: 1,
            turns: VecDeque::new(),
            transcript_bytes: 0,
            next_server_seq: 1,
            source_last_seq: BTreeMap::new(),
            source_latest: BTreeMap::new(),
            barriers: BTreeSet::new(),
            ambiguous: BTreeSet::new(),
            queue: VecDeque::new(),
            lunch: if mode == Mode::Lunch {
                Some(LunchState::default())
            } else {
                None
            },
            last_publish_ms: None,
            memory_seen: 0,
            eval_failed: None,
            context: None,
            context_history: VecDeque::new(),
            context_proposals: 0,
            auto_goals: false,
            stated_goals: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Snapshots and envelopes
    // ------------------------------------------------------------------

    pub fn header(&self, request_id: &str, kind: &str) -> Header {
        Header {
            protocol: PROTOCOL.into(),
            request_id: request_id.into(),
            instance_id: self.instance_id.clone(),
            kind: kind.into(),
            session_id: Some(self.id.clone()),
            mode: Some(self.mode),
            rev: self.rev,
            state_rev: self.state_rev,
            generation: self.generation,
            error: None,
        }
    }

    pub fn latest_turn(&self) -> Option<&TurnRecord> {
        self.turns.back()
    }

    pub fn turn(&self, seq: u64) -> Option<&TurnRecord> {
        self.turns.iter().find(|t| t.server_seq == seq)
    }
    fn turn_mut(&mut self, seq: u64) -> Option<&mut TurnRecord> {
        self.turns.iter_mut().find(|t| t.server_seq == seq)
    }

    pub fn snapshot(&self, request_id: &str) -> CueSnapshot {
        let d = self.decision.as_ref();
        let outcome = d.map(|d| d.outcome).unwrap_or(Outcome::None);
        let (cue, actionable) = match self.phase {
            Phase::Idle => (None, false),
            Phase::Processing | Phase::Invalidated | Phase::Closed => (Some(Cue::clear()), true),
            Phase::Unavailable => (Some(Cue::error()), true),
            Phase::Decided | Phase::AwaitingConfirmation => (
                Some(Cue::for_outcome(
                    outcome,
                    d.and_then(|d| d.selected_option.as_deref()),
                )),
                true,
            ),
        };
        // provenance and timings belong to the current generation: a voice-context
        // update (timings unmeasured -> null) or the turn with that server_seq
        let gen_ctx = self.context_by_seq(self.generation);
        let lt = if gen_ctx.is_some() {
            None
        } else {
            self.turn(self.generation)
        };
        let mut header = self.header(request_id, "cue_snapshot");
        header.error = d.and_then(|d| d.error.clone());
        let mut s = CueSnapshot {
            header,
            current: true,
            phase: self.phase,
            outcome,
            selected_option: if actionable {
                d.and_then(|d| d.selected_option.clone())
            } else {
                None
            },
            cue,
            actionable,
            reason: d.map(|d| d.reason.clone()).or_else(|| match self.phase {
                Phase::Processing => Some("Processing the latest turn.".into()),
                Phase::Invalidated => Some("State changed; re-evaluating.".into()),
                Phase::Closed => Some("Session closed.".into()),
                _ => None,
            }),
            missing: d.map(|d| d.missing.clone()).unwrap_or_default(),
            conflicts: d.map(|d| d.conflicts.clone()).unwrap_or_default(),
            unconfirmed: d.map(|d| d.unconfirmed.clone()).unwrap_or_default(),
            conditions: d.map(|d| d.conditions.clone()).unwrap_or_default(),
            barriers: self.barriers.iter().copied().collect(),
            truncated: false,
            provenance: Provenance {
                input_source: match gen_ctx {
                    Some(c) => c.input_source.clone(),
                    None => lt
                        .map(|t| t.input_source.clone())
                        .unwrap_or_else(|| "none".into()),
                },
                transcript_source: match gen_ctx {
                    Some(c) => Some(c.transcript_source.clone()),
                    None => lt.map(|t| t.transcript_source.clone()),
                },
                providers: self.providers.clone(),
                pipeline: "rust".into(),
                decision_source: d
                    .map(|d| d.decision_source.clone())
                    .unwrap_or_else(|| "none".into()),
            },
            timings_ms: lt.map(|t| t.timings.clone()).unwrap_or_default(),
        };
        bound_snapshot(&mut s);
        s
    }

    pub fn turn_ref(t: &TurnRecord) -> TurnRef {
        TurnRef {
            source_id: t.source_id.clone(),
            client_turn_id: t.client_turn_id.clone(),
            source_seq: t.source_seq,
            server_seq: Some(t.server_seq),
            attempt: t.attempt,
            attempts_total: t.attempts_total,
        }
    }

    pub fn turn_outcome(t: &TurnRecord) -> TurnOutcome {
        match t.status {
            TurnStatus::Queued
            | TurnStatus::Extracting
            | TurnStatus::Applied
            | TurnStatus::Evaluating => TurnOutcome::Pending,
            TurnStatus::Completed { published: true } => TurnOutcome::Applied,
            TurnStatus::Completed { published: false } => TurnOutcome::AppliedSuperseded,
            TurnStatus::FailedBeforeApply => TurnOutcome::FailedBeforeApply,
            TurnStatus::FailedAfterApply => TurnOutcome::FailedAfterApply,
            TurnStatus::Cancelled => TurnOutcome::Cancelled,
            TurnStatus::Discarded => TurnOutcome::Discarded,
        }
    }

    /// Canonical turn result: always the turn's latest attempt.
    pub fn turn_result(&self, seq: u64, request_id: &str) -> Option<TurnResult> {
        let t = self.turn(seq)?;
        let outcome = Self::turn_outcome(t);
        let snapshot = if outcome == TurnOutcome::Applied && self.generation == seq {
            Some(self.snapshot(request_id))
        } else {
            None
        };
        let mut header = self.header(request_id, "turn_result");
        header.error = t.error.clone();
        Some(TurnResult {
            header,
            turn: Self::turn_ref(t),
            turn_outcome: outcome,
            turn_effect: t.effect.clone(),
            snapshot,
        })
    }

    pub fn find_turn(&self, source_id: &str, client_turn_id: &str) -> Option<u64> {
        self.turns
            .iter()
            .find(|t| t.source_id == source_id && t.client_turn_id == client_turn_id)
            .map(|t| t.server_seq)
    }

    pub fn session_info(&self, request_id: &str) -> SessionInfo {
        SessionInfo {
            header: self.header(request_id, "session_info"),
            phase: self.phase,
            latest_generation: self.generation,
            source_latest: self
                .source_latest
                .iter()
                .map(|(s, (c, q))| SourceLatest {
                    source_id: s.clone(),
                    client_turn_id: c.clone(),
                    server_seq: *q,
                })
                .collect(),
            cue: None,
            actionable: false,
        }
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    pub(crate) fn bump_state(&mut self) {
        self.state_rev += 1;
    }

    /// Invalidate the standing decision immediately (LED clear) and queue re-evaluation.
    pub(crate) fn invalidate(&mut self) {
        self.decision = None;
        self.phase = if self.closed {
            Phase::Closed
        } else {
            Phase::Invalidated
        };
        self.rev += 1;
        self.enqueue_eval(None);
    }

    pub fn enqueue_eval(&mut self, trigger: Option<u64>) {
        let job = Job::Evaluate { trigger };
        if !self.queue.contains(&job) {
            self.queue.push_back(job);
        }
    }

    pub fn pop_job(&mut self) -> Option<Job> {
        self.queue.pop_front()
    }

    fn later_turn_pending(&self, seq: Option<u64>) -> bool {
        self.turns
            .iter()
            .any(|t| t.status.is_pre_apply_pending() && seq.is_none_or(|s| t.server_seq > s))
    }

    /// Evict terminal turn records beyond the retention limits. Turns that still
    /// carry an unresolved barrier or ambiguity are never evicted (they must stay
    /// discardable); admission is capped instead (`MAX_UNRESOLVED_TURNS`).
    fn evict(&mut self) {
        while self.turns.len() > TURN_RECORDS_PER_SESSION
            || self.transcript_bytes > TRANSCRIPT_BYTES_PER_SESSION
        {
            let Some(pos) = self.turns.iter().position(|t| {
                t.status.is_terminal()
                    && !self.barriers.contains(&t.server_seq)
                    && !self.ambiguous.contains(&t.server_seq)
            }) else {
                break;
            };
            if let Some(t) = self.turns.remove(pos) {
                self.transcript_bytes = self.transcript_bytes.saturating_sub(t.text.len());
            }
        }
    }

    fn active_terms_summary(&self) -> String {
        match &self.active {
            None => "none".into(),
            Some(o) => {
                let parts: Vec<String> = o.facts.iter().map(|f| f.stmt.describe()).collect();
                format!(
                    "{}: {}",
                    o.id,
                    if parts.is_empty() {
                        "no terms".into()
                    } else {
                        parts.join("; ")
                    }
                )
            }
        }
    }

    fn slot_versions(&self) -> BTreeMap<Slot, Vec<(u64, u64)>> {
        let mut m: BTreeMap<Slot, Vec<(u64, u64)>> = BTreeMap::new();
        if let Some(o) = &self.active {
            for f in &o.facts {
                m.entry(f.stmt.slot()).or_default().push((f.id, f.version));
            }
        }
        m
    }

    // ------------------------------------------------------------------
    // Turn admission
    // ------------------------------------------------------------------

    pub fn accept_turn(&mut self, r: AcceptReq) -> Result<Accepted, ApiError> {
        if self.closed {
            return Err(ApiError::new("session_closed", "session is closed", false));
        }
        if r.client_turn_id.is_empty()
            || r.client_turn_id.len() > CLIENT_TURN_ID_MAX
            || r.source_id.len() > SOURCE_ID_MAX
            || r.transcript_source.len() > LABEL_MAX
            || r.speaker.as_ref().is_some_and(|s| s.len() > LABEL_MAX)
        {
            return Err(ApiError::new(
                "field_too_long",
                "client_turn_id, source_id, transcript_source or speaker exceeds its limit",
                false,
            ));
        }
        if let Some(t) = self
            .turns
            .iter()
            .find(|t| t.source_id == r.source_id && t.client_turn_id == r.client_turn_id)
        {
            let same = t.mode == r.mode
                && r.source_seq.is_none_or(|s| s == t.source_seq)
                && t.text == r.text.trim()
                && t.transcript_source == r.transcript_source
                && t.speaker == r.speaker;
            return if same {
                Ok(Accepted::Duplicate(t.server_seq))
            } else {
                Err(ApiError::new(
                    "turn_id_reused",
                    "client_turn_id reused with different content",
                    false,
                ))
            };
        }
        if r.mode != self.mode {
            return Err(ApiError::new(
                "mode_mismatch",
                "turn mode does not match the session",
                false,
            ));
        }
        let last = self.source_last_seq.get(&r.source_id).copied().unwrap_or(0);
        let source_seq = match r.source_seq {
            Some(s) if s <= last => {
                return Err(ApiError::new(
                    "stale_turn",
                    format!("source_seq {s} <= last accepted {last} for {}", r.source_id),
                    false,
                ))
            }
            Some(s) => s,
            None => last + 1,
        };
        let text = r.text.trim().to_string();
        if text.is_empty() {
            return Err(ApiError::new(
                "empty_transcript",
                "transcript is empty",
                false,
            ));
        }
        if text.chars().count() > TRANSCRIPT_MAX_CHARS {
            return Err(ApiError::new(
                "transcript_too_long",
                "transcript exceeds 4000 characters",
                false,
            ));
        }
        if !self.goals_confirmed && self.mode == Mode::Negotiation {
            return Err(ApiError::new(
                "goals_unconfirmed",
                "confirm goals before submitting turns",
                false,
            ));
        }
        if self.mode == Mode::Lunch && !self.lunch.as_ref().is_some_and(|l| l.setup_confirmed) {
            return Err(ApiError::new(
                "goals_unconfirmed",
                "confirm the lunch window and three options first",
                false,
            ));
        }
        if self.barriers.len() + self.ambiguous.len() >= MAX_UNRESOLVED_TURNS {
            return Err(ApiError::new(
                "too_many_unresolved_turns",
                "retry, discard or resolve unprocessed and unclear turns first",
                true,
            ));
        }
        let pending = self
            .turns
            .iter()
            .filter(|t| t.status.is_pre_apply_pending())
            .count();
        if pending > TURN_QUEUE_MAX || self.queue.len() >= SESSION_JOB_QUEUE_MAX {
            return Err(ApiError::new(
                "turn_queue_full",
                "too many turns waiting; retry shortly",
                true,
            ));
        }
        let seq = self.next_server_seq;
        self.next_server_seq += 1;
        self.source_last_seq.insert(r.source_id.clone(), source_seq);
        self.source_latest
            .insert(r.source_id.clone(), (r.client_turn_id.clone(), seq));
        self.transcript_bytes += text.len();
        self.turns.push_back(TurnRecord {
            server_seq: seq,
            source_id: r.source_id,
            client_turn_id: r.client_turn_id,
            source_seq,
            mode: r.mode,
            text,
            transcript_source: r.transcript_source,
            speaker: r.speaker,
            input_source: r.input_source,
            status: TurnStatus::Queued,
            attempt: 1,
            attempts_total: 1,
            effect_applied: false,
            effect: None,
            error: None,
            rejected_terms: vec![],
            accepted_ms: r.now_ms,
            timings: Timings::default(),
            awaiting_since_ms: None,
            human_wait_ms: 0,
            pending_terms: vec![],
        });
        self.generation = seq;
        // LED clear before extraction finishes (PLAN v2 §5.3, v4 R9)
        self.decision = None;
        self.phase = Phase::Processing;
        self.rev += 1;
        self.queue.push_back(Job::Turn { seq });
        self.evict();
        Ok(Accepted::New(seq))
    }

    // ------------------------------------------------------------------
    // Phase A
    // ------------------------------------------------------------------

    /// Begin extraction. A turn that waited in the queue longer than
    /// `QUEUE_WAIT_MAX_MS` fails with `queue_timeout` instead.
    pub fn start_extraction(
        &mut self,
        seq: u64,
        now_ms: u64,
        deadline_ms: u64,
    ) -> Option<ExtractCtx> {
        let waited = self
            .turn(seq)
            .map(|t| now_ms.saturating_sub(t.accepted_ms))
            .unwrap_or(0);
        if self
            .turn(seq)
            .is_some_and(|t| t.status == TurnStatus::Queued)
            && waited > QUEUE_WAIT_MAX_MS
        {
            self.fail_before_apply(
                seq,
                ApiError::new("queue_timeout", "turn waited too long in the queue", true),
                now_ms,
            );
            return None;
        }
        let active_terms = self.active_terms_summary();
        let slot_versions = self.slot_versions();
        let active_offer_id = self.active.as_ref().map(|o| o.id.clone());
        let (lunch_option_versions, lunch_prompt) = match &self.lunch {
            Some(l) => (l.option_versions(), l.prompt_context()),
            None => ([0; 3], String::new()),
        };
        let mode = self.mode;
        let closed = self.closed;
        let t = self.turn_mut(seq)?;
        if closed || t.status != TurnStatus::Queued {
            return None;
        }
        t.status = TurnStatus::Extracting;
        Some(ExtractCtx {
            seq,
            attempt: t.attempt,
            mode,
            text: t.text.clone(),
            active_offer_id,
            active_terms,
            slot_versions,
            lunch_option_versions,
            lunch_prompt,
            reextracted: false,
            deadline_ms,
        })
    }

    /// Fresh snapshot for a single re-extraction after a version conflict.
    pub fn reextract_ctx(&mut self, seq: u64, deadline_ms: u64) -> Option<ExtractCtx> {
        let t = self.turn(seq)?;
        if t.status != TurnStatus::Extracting || self.closed {
            return None;
        }
        let (lunch_option_versions, lunch_prompt) = match &self.lunch {
            Some(l) => (l.option_versions(), l.prompt_context()),
            None => ([0; 3], String::new()),
        };
        Some(ExtractCtx {
            seq,
            attempt: t.attempt,
            mode: self.mode,
            text: t.text.clone(),
            active_offer_id: self.active.as_ref().map(|o| o.id.clone()),
            active_terms: self.active_terms_summary(),
            slot_versions: self.slot_versions(),
            lunch_option_versions,
            lunch_prompt,
            reextracted: true,
            deadline_ms,
        })
    }

    pub fn set_extract_ms(&mut self, seq: u64, ms: u64) {
        if let Some(t) = self.turn_mut(seq) {
            t.timings.extract = Some(ms);
        }
    }
    pub fn set_evaluate_ms(&mut self, seq: Option<u64>, ms: u64) {
        if let Some(t) = seq.and_then(|s| self.turn_mut(s)) {
            t.timings.evaluate = Some(ms);
        }
    }

    /// Lunch setup changes (option names, window, user facts) by the UI.
    pub fn lunch_setup_changed(&mut self) {
        self.bump_state();
        self.invalidate();
    }

    /// Memory publication / pending change (called under the memory publication
    /// lock): remember the version and invalidate any lunch decision.
    pub fn memory_changed(&mut self, publication: u64) {
        self.memory_seen = self.memory_seen.max(publication);
        if self.mode == Mode::Lunch && !self.closed {
            self.invalidate();
        }
    }

    /// Extraction failed before facts were applied: barrier + UNAVAILABLE.
    pub fn fail_before_apply(&mut self, seq: u64, err: ApiError, now_ms: u64) {
        let Some(t) = self.turn_mut(seq) else { return };
        if t.status.is_terminal() {
            return;
        }
        t.status = TurnStatus::FailedBeforeApply;
        t.error = Some(err.clone());
        self.barriers.insert(seq);
        self.bump_state();
        if !self.later_turn_pending(Some(seq)) {
            let mut d = Decision::simple(
                Outcome::Unavailable,
                format!("Turn {seq} was not processed: {}", err.message),
                "none",
            );
            d.error = Some(err);
            self.publish(d, Some(seq), now_ms);
        }
    }

    /// Evaluation failed after facts were applied. A failure of a user-triggered
    /// evaluation (no turn) is recorded so it can be retried without changing facts.
    pub fn fail_after_apply(&mut self, seq: Option<u64>, err: ApiError, now_ms: u64) {
        match seq {
            Some(seq) => {
                if let Some(t) = self.turn_mut(seq) {
                    if !t.status.is_terminal() || t.status == TurnStatus::FailedAfterApply {
                        t.status = TurnStatus::FailedAfterApply;
                        t.error = Some(err.clone());
                    }
                }
            }
            None => self.eval_failed = Some(err.clone()),
        }
        if !self.later_turn_pending(seq) {
            let mut d = Decision::simple(
                Outcome::Unavailable,
                format!("No decision: {}", err.message),
                "none",
            );
            d.error = Some(err);
            self.publish(d, seq, now_ms);
        }
    }

    fn offer_id(&self, n: u64) -> String {
        // session-qualified so IDs never repeat across sessions
        format!("{}:o{n}", self.id)
    }

    /// Apply a validated negotiation extraction atomically (Phase A).
    pub fn apply_negotiation(
        &mut self,
        ctx: &ExtractCtx,
        ext: &NegExtraction,
        extractor: &str,
        now_ms: u64,
    ) -> PhaseA {
        let Some(t) = self.turn(ctx.seq) else {
            return PhaseA::Dropped;
        };
        if self.closed || t.status != TurnStatus::Extracting || t.effect_applied {
            return PhaseA::Dropped;
        }
        if now_ms > ctx.deadline_ms {
            return PhaseA::Expired;
        }
        if ext.terms.len() > MAX_EXTRACTED_TERMS {
            return PhaseA::Invalid(ApiError::new(
                "extraction_invalid_output",
                format!(
                    "{} terms exceed the {MAX_EXTRACTED_TERMS}-term contract; nothing applied",
                    ext.terms.len()
                ),
                false,
            ));
        }
        let transcript = t.text.clone();
        // validate against the FULL transcript, then apply in-turn corrections
        let mut valid = Vec::new();
        let mut rejected: Vec<RejectedTerm> = Vec::new();
        for raw in &ext.terms {
            match validate_term(raw, &transcript) {
                Ok(v) => valid.push(v),
                Err(r) => rejected.push(r),
            }
        }
        let valid = apply_in_turn_corrections(valid, &transcript);
        let current_offer = self.active.as_ref().map(|o| o.id.clone());
        let offer_changed = current_offer != ctx.active_offer_id;
        let lower = transcript.to_lowercase();
        let hypothetical = [
            "what if",
            "would you",
            "could you",
            "suppose",
            "if we",
            "if i",
        ]
        .iter()
        .any(|m| lower.contains(m))
            || lower.trim_end().ends_with('?');
        let mut kind = ext.turn_kind;
        // deterministic guard: explicit "same offer" / "keep all other terms" wording
        // must never replace the active offer; a conflicting new_offer reading is
        // treated as unclear (ASK) instead
        if kind == TurnKind::NewOffer && current_offer.is_some() && keeps_other_terms(&transcript) {
            kind = TurnKind::Ambiguous;
        }
        if matches!(kind, TurnKind::NewOffer | TurnKind::Amendment)
            && (valid.is_empty() || hypothetical)
        {
            kind = TurnKind::Ambiguous;
        }
        if kind == TurnKind::Amendment
            && (current_offer.is_none()
                || ext.amends_offer_id.as_deref() != current_offer.as_deref())
        {
            kind = TurnKind::Ambiguous;
        }
        // version checks on affected slots (amendment) or offer identity (new offer)
        if offer_changed {
            return if ctx.reextracted {
                self.mark_ambiguous(
                    ctx.seq,
                    &valid,
                    &rejected,
                    "offer changed during extraction",
                )
            } else {
                PhaseA::Reextract
            };
        }
        if kind == TurnKind::Amendment {
            let now = self.slot_versions();
            let touched: BTreeSet<Slot> = valid.iter().map(|v| v.stmt.slot()).collect();
            let changed = touched
                .iter()
                .any(|s| now.get(s) != ctx.slot_versions.get(s));
            if changed {
                return if ctx.reextracted {
                    self.mark_ambiguous(
                        ctx.seq,
                        &valid,
                        &rejected,
                        "terms changed during extraction",
                    )
                } else {
                    PhaseA::Reextract
                };
            }
            let touched_count = self
                .active
                .as_ref()
                .map(|o| {
                    o.facts
                        .iter()
                        .filter(|f| !touched.contains(&f.stmt.slot()))
                        .count()
                })
                .unwrap_or(0);
            if touched_count + valid.len() > FACTS_PER_OFFER {
                // never silently drop evidence to satisfy a limit
                return self.mark_ambiguous(ctx.seq, &valid, &rejected, "offer fact limit reached");
            }
        }
        let seq = ctx.seq;
        let mk_fact = |id: u64, v: &ValidatedTerm| Fact {
            id,
            version: 1,
            stmt: v.stmt.clone(),
            confirmed: false,
            source_seq: Some(seq),
            excerpt: v.excerpt.clone(),
            excerpt_truncated: v.excerpt_truncated,
            byte_range: v.byte_range,
            extractor: extractor.to_string(),
        };
        let effect = match kind {
            TurnKind::NewOffer => {
                let id = self.offer_id(self.next_offer_no);
                self.next_offer_no += 1;
                let mut facts = Vec::new();
                for v in valid.iter() {
                    facts.push(mk_fact(self.next_fact_id, v));
                    self.next_fact_id += 1;
                }
                if let Some(mut old) = self.active.take() {
                    for c in old.conditions.iter_mut() {
                        if c.status == ConditionStatus::Unresolved {
                            c.status = ConditionStatus::SupersededByOffer;
                        }
                    }
                    self.archive(old);
                }
                self.active = Some(Offer {
                    id,
                    version: 1,
                    facts,
                    conditions: vec![],
                    created_seq: seq,
                    turn_seqs: vec![seq],
                    resolved_through_seq: seq.saturating_sub(1),
                    dropped_through: 0,
                    unresolved: vec![],
                });
                // replacement-offer exception: earlier barriers and ambiguities cleared
                self.barriers.retain(|b| *b > seq);
                self.ambiguous.retain(|a| *a > seq);
                "new_offer"
            }
            TurnKind::Amendment => {
                let mut next_id = self.next_fact_id;
                let o = self.active.as_mut().expect("checked");
                let touched: BTreeSet<Slot> = valid.iter().map(|v| v.stmt.slot()).collect();
                o.facts.retain(|f| !touched.contains(&f.stmt.slot()));
                for v in &valid {
                    o.facts.push(mk_fact(next_id, v));
                    next_id += 1;
                }
                o.version += 1;
                o.push_turn(seq);
                self.next_fact_id = next_id;
                "amendment"
            }
            TurnKind::QuestionOrHypothetical | TurnKind::Irrelevant => {
                if let Some(o) = self.active.as_mut() {
                    o.push_turn(seq);
                }
                "no_change"
            }
            TurnKind::Ambiguous => {
                return self.mark_ambiguous(seq, &valid, &rejected, "unclear statement")
            }
        };
        // rejected critical interpretations of an offer-changing turn stay open
        let blocking: Vec<&RejectedTerm> = if effect == "no_change" {
            vec![]
        } else {
            rejected.iter().filter(|r| !r.context_only).collect()
        };
        let mut added_unresolved = false;
        if !blocking.is_empty() {
            if let Some(o) = self.active.as_mut() {
                for r in blocking {
                    o.record_unresolved(Unresolved {
                        id: self.next_cond_id,
                        version: 1,
                        field: format!("{:?}", r.field).to_lowercase(),
                        reason: bounded(&r.reason, 200).0,
                        excerpt: r.excerpt.clone(),
                        source_seq: seq,
                        open: true,
                        merged: 0,
                    });
                    self.next_cond_id += 1;
                    added_unresolved = true;
                }
                o.version += 1;
            }
        }
        let rejected_text = rejected_lines(&rejected);
        let Some(t) = self.turn_mut(seq) else {
            return PhaseA::Dropped;
        };
        t.status = TurnStatus::Applied;
        t.effect_applied = true;
        t.effect = Some(effect.into());
        t.rejected_terms = rejected_text;
        self.barriers.remove(&seq);
        // facts/offers changed only for new offers and amendments; a no-change turn
        // does not make open confirmation forms stale (the worker is serial, so no
        // evaluation can interleave with this apply)
        if effect != "no_change" || added_unresolved {
            self.bump_state();
        }
        PhaseA::Applied
    }

    fn mark_ambiguous(
        &mut self,
        seq: u64,
        valid: &[ValidatedTerm],
        rejected: &[RejectedTerm],
        why: &str,
    ) -> PhaseA {
        if let Some(o) = self.active.as_mut() {
            o.push_turn(seq);
        }
        let mut lines = rejected_lines(rejected);
        if lines.len() < LIST_MAX_ENTRIES {
            lines.push(format!("unclear: {why}"));
        }
        let pending: Vec<(Stmt, String, bool)> = valid
            .iter()
            .take(MAX_EXTRACTED_TERMS)
            .map(|v| (v.stmt.clone(), v.excerpt.clone(), v.excerpt_truncated))
            .collect();
        let Some(t) = self.turn_mut(seq) else {
            return PhaseA::Dropped;
        };
        t.status = TurnStatus::Applied;
        t.effect_applied = true;
        t.effect = Some("ambiguous".into());
        t.rejected_terms = lines;
        t.pending_terms = pending;
        self.ambiguous.insert(seq);
        self.barriers.remove(&seq);
        self.bump_state();
        PhaseA::Applied
    }

    fn archive(&mut self, old: Offer) {
        self.archived.push_back(old);
        while self.archived.len() > OFFERS_PER_SESSION {
            if let Some(o) = self.archived.pop_front() {
                let summary: Vec<String> = o.facts.iter().map(|f| f.stmt.describe()).collect();
                let (s, _) = crate::text::bounded(&summary.join("; "), 256);
                self.tombstones.push_back(Tombstone {
                    id: o.id,
                    created_seq: o.created_seq,
                    summary: s,
                });
                while self.tombstones.len() > TOMBSTONES_PER_SESSION {
                    self.tombstones.pop_front();
                }
            }
        }
    }

    pub fn apply_lunch(
        &mut self,
        ctx: &ExtractCtx,
        ext: &lunch::LunchExtraction,
        extractor: &str,
        now_ms: u64,
    ) -> PhaseA {
        let Some(t) = self.turn(ctx.seq) else {
            return PhaseA::Dropped;
        };
        if self.closed || t.status != TurnStatus::Extracting || t.effect_applied {
            return PhaseA::Dropped;
        }
        if now_ms > ctx.deadline_ms {
            return PhaseA::Expired;
        }
        if ext.facts.len() > MAX_EXTRACTED_TERMS {
            return PhaseA::Invalid(ApiError::new(
                "extraction_invalid_output",
                format!(
                    "{} facts exceed the {MAX_EXTRACTED_TERMS}-fact contract; nothing applied",
                    ext.facts.len()
                ),
                false,
            ));
        }
        let transcript = t.text.clone();
        let seq = ctx.seq;
        let Some(l) = self.lunch.as_mut() else {
            return PhaseA::Dropped;
        };
        let (valid, rejected) = lunch::validate_extraction(ext, &transcript, &l.options);
        let touched: BTreeSet<usize> = valid.iter().map(|v| v.option).collect();
        let now = l.option_versions();
        if touched
            .iter()
            .any(|i| now[*i] != ctx.lunch_option_versions[*i])
        {
            if ctx.reextracted {
                return self.mark_ambiguous(seq, &[], &[], "options changed during extraction");
            }
            return PhaseA::Reextract;
        }
        if ext.turn_kind == lunch::LunchTurnKind::Ambiguous {
            return self.mark_ambiguous(seq, &[], &[], "unclear statement");
        }
        if !l.can_add(&valid) {
            return self.mark_ambiguous(seq, &[], &[], "option fact limit reached");
        }
        let fact_turn = matches!(ext.turn_kind, lunch::LunchTurnKind::OptionFacts);
        let mut changed = false;
        if fact_turn {
            for r in &rejected {
                changed |= l.add_unresolved(r, seq);
            }
        }
        let effect = if valid.is_empty() {
            "no_change"
        } else {
            l.apply_facts(valid, seq, extractor);
            changed = true;
            "facts_added"
        };
        let lines: Vec<String> = rejected
            .iter()
            .take(LIST_MAX_ENTRIES)
            .map(|r| bounded(&format!("{} ({})", r.reason, r.excerpt), 240).0)
            .collect();
        let Some(t) = self.turn_mut(seq) else {
            return PhaseA::Dropped;
        };
        t.status = TurnStatus::Applied;
        t.effect_applied = true;
        t.effect = Some(effect.into());
        t.rejected_terms = lines;
        self.barriers.remove(&seq);
        if changed {
            self.bump_state();
        }
        PhaseA::Applied
    }

    // ------------------------------------------------------------------
    // Phase B
    // ------------------------------------------------------------------

    fn publish(&mut self, d: Decision, trigger: Option<u64>, now_ms: u64) {
        self.phase = match d.outcome {
            Outcome::Unavailable => Phase::Unavailable,
            Outcome::Ask if d.awaiting_confirmation => Phase::AwaitingConfirmation,
            _ => Phase::Decided,
        };
        if self.closed {
            self.phase = Phase::Closed;
        }
        if d.error.is_none() && trigger.is_none() {
            self.eval_failed = None;
        }
        let awaiting = d.awaiting_confirmation;
        self.decision = Some(d);
        self.rev += 1;
        self.last_publish_ms = Some(now_ms);
        if let Some(seq) = trigger {
            if let Some(t) = self.turn_mut(seq) {
                if t.timings.accept_to_publish.is_none() {
                    t.timings.accept_to_publish = Some(now_ms.saturating_sub(t.accepted_ms));
                }
                if matches!(t.status, TurnStatus::Applied | TurnStatus::Evaluating) {
                    t.status = TurnStatus::Completed { published: true };
                }
            }
        }
        // generation timing: every publication (including confirmation-triggered
        // evaluations) is attributed to the current generation's turn
        let generation = self.generation;
        if let Some(t) = self.turn_mut(generation) {
            let e2e = now_ms.saturating_sub(t.accepted_ms);
            t.timings.e2e = Some(e2e);
            t.timings.human_wait = Some(t.human_wait_ms);
            t.timings.processing = Some(e2e.saturating_sub(t.human_wait_ms));
            t.awaiting_since_ms = if awaiting { Some(now_ms) } else { None };
        }
    }

    /// The user acted on a confirmation request: count the wait as human time.
    fn record_human_wait(&mut self, now_ms: u64) {
        let generation = self.generation;
        if let Some(t) = self.turn_mut(generation) {
            if let Some(since) = t.awaiting_since_ms.take() {
                t.human_wait_ms += now_ms.saturating_sub(since);
            }
        }
    }

    fn mark_superseded(&mut self, seq: u64) {
        if let Some(t) = self.turn_mut(seq) {
            if matches!(t.status, TurnStatus::Applied | TurnStatus::Evaluating) {
                t.status = TurnStatus::Completed { published: false };
            }
        }
    }

    /// Start Phase B. Pure rules run here; a contextual request is returned when needed.
    pub fn start_evaluation(
        &mut self,
        trigger: Option<u64>,
        memory: Option<&lunch::MemoryView>,
        now_ms: u64,
        deadline_ms: u64,
    ) -> EvalStart {
        if self.closed {
            return EvalStart::Skipped;
        }
        if now_ms > deadline_ms {
            return EvalStart::Expired;
        }
        // a lunch evaluation must use the newest published memory version
        // (no re-enqueue: the publication hook already queued an evaluation)
        if self.mode == Mode::Lunch && memory.is_some_and(|m| m.publication < self.memory_seen) {
            return EvalStart::Skipped;
        }
        if self.later_turn_pending(trigger) {
            if let Some(seq) = trigger {
                self.mark_superseded(seq);
                return EvalStart::Superseded;
            }
            return EvalStart::Skipped;
        }
        if let Some(seq) = trigger {
            if let Some(t) = self.turn_mut(seq) {
                if t.status == TurnStatus::Applied || t.status == TurnStatus::FailedAfterApply {
                    t.status = TurnStatus::Evaluating;
                }
            }
        }
        let rules = match self.mode {
            Mode::Negotiation => self.negotiation_rules(),
            Mode::Lunch => self.lunch_rules(memory),
        };
        match rules {
            Rules::Final(d, conds) => {
                self.add_conditions(conds);
                self.publish(d, trigger, now_ms);
                EvalStart::Committed
            }
            Rules::Idle => {
                self.decision = None;
                if self.phase != Phase::Idle {
                    self.phase = Phase::Idle;
                    self.rev += 1;
                }
                if let Some(seq) = trigger {
                    self.mark_superseded(seq);
                }
                EvalStart::Committed
            }
            Rules::Contextual {
                base,
                request,
                covers,
            } => EvalStart::NeedsContextual(Box::new(EvalCtx {
                trigger,
                state_rev: self.state_rev,
                memory_version: memory.map(|m| m.publication).unwrap_or(0),
                offer_id: self.active.as_ref().map(|o| o.id.clone()),
                covers,
                request,
                base,
                deadline_ms,
            })),
        }
    }

    fn add_conditions(&mut self, conds: Vec<(ConditionKind, (u64, u64), String)>) {
        if conds.is_empty() {
            return;
        }
        let mut next = self.next_cond_id;
        let mut added = false;
        if let Some(o) = self.active.as_mut() {
            for (kind, covers, note) in conds {
                if covers.1 <= o.resolved_through_seq {
                    continue; // stale observation of already-resolved turns
                }
                let dup = o.conditions.iter().any(|c| {
                    c.status == ConditionStatus::Unresolved && c.kind == kind && c.covers == covers
                });
                if dup {
                    continue;
                }
                if o.record_condition(Condition {
                    id: next,
                    version: 1,
                    kind,
                    covers,
                    status: ConditionStatus::Unresolved,
                    note,
                }) {
                    next += 1;
                    o.version += 1;
                    added = true;
                }
            }
        }
        self.next_cond_id = next;
        if added {
            self.bump_state();
        }
    }

    /// Commit Phase B atomically: validate the snapshot, then record the job's
    /// conditions and publish its decision together.
    pub fn commit_contextual(
        &mut self,
        ctx: EvalCtx,
        result: ContextualResult,
        memory: Option<&lunch::MemoryView>,
        decide_ms: u64,
        now_ms: u64,
    ) -> Commit {
        if self.closed {
            return Commit::Stale;
        }
        if now_ms > ctx.deadline_ms {
            return Commit::Expired;
        }
        if self.later_turn_pending(ctx.trigger) {
            if let Some(seq) = ctx.trigger {
                self.mark_superseded(seq);
            }
            return Commit::Superseded;
        }
        // re-checked under the commit lock: a pending context update makes this result stale
        if self.context_open() {
            return Commit::Stale;
        }
        let mem_ok = memory.is_none_or(|m| {
            m.publication == ctx.memory_version
                && m.publication >= self.memory_seen
                && !m.pending_change
                && m.recovery_required.is_none()
        });
        if self.mode == Mode::Lunch && memory.is_none() {
            return Commit::Stale;
        }
        if ctx.state_rev != self.state_rev
            || !mem_ok
            || self.active.as_ref().map(|o| o.id.clone()) != ctx.offer_id
        {
            return Commit::Stale;
        }
        if let Some(seq) = ctx.trigger {
            if let Some(t) = self.turn_mut(seq) {
                t.timings.decide = Some(decide_ms);
            }
        }
        let mut d = ctx.base.clone();
        let mut conds = Vec::new();
        match result {
            ContextualResult::Noul(Ok(v)) => match v {
                NoulVerdict::Clear { p_yes } => {
                    d.outcome = Outcome::Proceed;
                    d.decision_source = "gemini_decisions".into();
                    d.reason = format!("{} Contextual check clear (p_yes {:.2}).", d.reason, p_yes);
                }
                NoulVerdict::Uncertain { p_yes } => {
                    d.outcome = Outcome::Ask;
                    d.decision_source = "gemini_decisions".into();
                    d.reason = format!(
                        "{} Contextual check uncertain (p_yes {:.2}); review the conversation.",
                        d.reason, p_yes
                    );
                    conds.push((
                        ConditionKind::ContextualUncertain,
                        ctx.covers,
                        format!("contextual check uncertain (p_yes {p_yes:.2})"),
                    ));
                }
                NoulVerdict::Concern { p_yes } => {
                    d.outcome = Outcome::Ask;
                    d.decision_source = "gemini_decisions".into();
                    d.reason = format!(
                        "{} Possible unaddressed fee or condition (p_yes {:.2}).",
                        d.reason, p_yes
                    );
                    conds.push((
                        ConditionKind::ContextualConcern,
                        ctx.covers,
                        format!("possible unaddressed condition (p_yes {p_yes:.2})"),
                    ));
                }
            },
            ContextualResult::Choice(Ok(a)) => {
                let l = self.lunch.as_ref();
                d = lunch::decide_with_choice(d, &a, l);
            }
            ContextualResult::Noul(Err(e)) | ContextualResult::Choice(Err(e)) => {
                d = Decision::simple(
                    Outcome::Unavailable,
                    format!("Contextual check unavailable: {}", e.message),
                    "none",
                );
                d.error = Some(e);
                if let Some(seq) = ctx.trigger {
                    if let Some(t) = self.turn_mut(seq) {
                        t.status = TurnStatus::FailedAfterApply;
                        t.error = d.error.clone();
                    }
                }
            }
        }
        if self.providers != "real" && d.decision_source == "gemini_decisions" {
            d.decision_source = "mock_decisions".into();
            d.reason = d.reason.replace("Gemini Decisions", "MOCK decisions");
        }
        if !conds.is_empty() {
            d.conditions.extend(conds.iter().map(|c| c.2.clone()));
        }
        self.add_conditions(conds);
        self.publish(d, ctx.trigger, now_ms);
        Commit::Committed
    }

    // ------------------------------------------------------------------
    // Rules
    // ------------------------------------------------------------------

    fn negotiation_rules(&self) -> Rules {
        if !self.goals_confirmed {
            return Rules::Idle;
        }
        let Some(o) = &self.active else {
            if !self.barriers.is_empty() {
                return Rules::Final(self.barrier_decision(), vec![]);
            }
            if !self.ambiguous.is_empty() {
                return Rules::Final(self.ambiguous_decision(), vec![]);
            }
            return Rules::Final(
                Decision::simple(Outcome::None, "No offer yet.", "rules"),
                vec![],
            );
        };
        let stmts = o.stmts();
        let r = evaluate_requirements(&stmts, &self.goals);
        let mut violations = Vec::new();
        let mut unknowns = Vec::new();
        let mut satisfied = Vec::new();
        for (name, e) in r.all() {
            match e {
                Eval::Violates(m) => violations.push(format!("{name}: {m}")),
                Eval::Unknown(m) => unknowns.push(format!("{name}: {m}")),
                Eval::Satisfies(m) => satisfied.push(m.clone()),
            }
        }
        let unconfirmed: Vec<String> = o
            .facts
            .iter()
            .filter(|f| !f.confirmed)
            .map(|f| f.stmt.describe())
            .collect();
        let conditions: Vec<String> = o
            .unresolved_conditions()
            .iter()
            .map(|c| c.note.clone())
            .collect();
        let mut base = Decision {
            outcome: Outcome::Ask,
            awaiting_confirmation: false,
            selected_option: None,
            reason: String::new(),
            missing: unknowns.clone(),
            conflicts: violations.clone(),
            unconfirmed: unconfirmed.clone(),
            conditions: conditions.clone(),
            decision_source: "rules".into(),
            error: None,
        };
        if !violations.is_empty() {
            base.outcome = Outcome::Pause;
            let mut reason = format!("Pause: {}.", violations.join("; "));
            if !unknowns.is_empty() {
                reason.push_str(&format!(" Also missing: {}.", unknowns.join("; ")));
            }
            if !unconfirmed.is_empty() {
                reason.push_str(" (based on unconfirmed extracted terms)");
            }
            base.reason = reason;
            return Rules::Final(base, vec![]);
        }
        if !self.barriers.is_empty() {
            let mut d = self.barrier_decision();
            d.missing = unknowns;
            d.unconfirmed = unconfirmed;
            return Rules::Final(d, vec![]);
        }
        // a pending voice-context update blocks positive cues (after PAUSE and barriers)
        if let Some(mut d) = self.context_gate() {
            d.missing.extend(unknowns);
            d.unconfirmed = unconfirmed;
            d.conditions = conditions;
            return Rules::Final(d, vec![]);
        }
        if !self.ambiguous.is_empty() {
            unknowns.push(format!(
                "unclear statement in turn(s) {}",
                self.ambiguous
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            base.missing = unknowns.clone();
        }
        for u in o.open_unresolved() {
            unknowns.push(format!(
                "unresolved {} interpretation: {} (\"{}\")",
                u.field, u.reason, u.excerpt
            ));
        }
        base.missing = unknowns.clone();
        if !unknowns.is_empty() {
            base.reason = format!("Ask: {}.", unknowns.join("; "));
            return Rules::Final(base, vec![]);
        }
        if !unconfirmed.is_empty() {
            base.awaiting_confirmation = true;
            base.reason = format!(
                "All required terms stated and within goals ({}); confirm {} extracted term(s).",
                satisfied.join(", "),
                unconfirmed.len()
            );
            return Rules::Final(base, vec![]);
        }
        if !conditions.is_empty() {
            base.reason = format!("Ask: unresolved condition(s): {}.", conditions.join("; "));
            return Rules::Final(base, vec![]);
        }
        // context window since the last resolution point
        let window_seqs: Vec<u64> = o
            .turn_seqs
            .iter()
            .copied()
            .filter(|s| *s > o.resolved_through_seq)
            .collect();
        let covers = (
            window_seqs.first().copied().unwrap_or(o.created_seq),
            window_seqs.last().copied().unwrap_or(o.created_seq),
        );
        let mut texts = Vec::new();
        let mut missing_turn = false;
        for s in &window_seqs {
            match self.turn(*s) {
                Some(t) => texts.push(t.text.clone()),
                None => missing_turn = true,
            }
        }
        let window = texts.join("\n");
        if missing_turn
            || o.dropped_through > o.resolved_through_seq
            || window_seqs.len() > CONTEXT_MAX_TURNS
            || window.chars().count() > CONTEXT_MAX_CHARS
        {
            base.reason =
                "Ask: offer conversation exceeds the verifiable context window; review it.".into();
            base.conditions.push("context limit reached".into());
            return Rules::Final(
                base,
                vec![(
                    ConditionKind::ContextLimit,
                    covers,
                    "context limit reached".into(),
                )],
            );
        }
        base.reason = format!("Confirmed: {}.", satisfied.join(", "));
        let terms = o
            .facts
            .iter()
            .map(|f| format!("- {}", f.stmt.describe()))
            .collect::<Vec<_>>()
            .join("\n");
        Rules::Contextual {
            base,
            request: ContextualRequest::Noul { terms, window },
            covers,
        }
    }

    fn barrier_decision(&self) -> Decision {
        let list = self
            .barriers
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut d = Decision::simple(
            Outcome::Unavailable,
            format!("Turn(s) {list} were not processed; retry or discard before a recommendation."),
            "rules",
        );
        d.error = Some(ApiError::new(
            "unresolved_failed_turn",
            format!("unresolved failed turn(s): {list}"),
            true,
        ));
        d
    }

    fn ambiguous_decision(&self) -> Decision {
        let list = self
            .ambiguous
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut d = Decision::simple(
            Outcome::Ask,
            format!("Ask: unclear statement in turn(s) {list}."),
            "rules",
        );
        d.missing = vec![format!("unclear turn(s) {list}")];
        d
    }

    fn lunch_rules(&self, memory: Option<&lunch::MemoryView>) -> Rules {
        let Some(l) = &self.lunch else {
            return Rules::Idle;
        };
        if !l.setup_confirmed {
            return Rules::Idle;
        }
        match lunch::rules(l, memory, &self.barriers, &self.ambiguous) {
            lunch::LunchRules::Final(d) => Rules::Final(d, vec![]),
            lunch::LunchRules::Choice {
                base,
                soft_prefs,
                options,
            } => Rules::Contextual {
                base,
                request: ContextualRequest::Choice {
                    soft_prefs,
                    options,
                },
                covers: (0, 0),
            },
        }
    }

    // ------------------------------------------------------------------
    // User actions (all bound to state_rev and object versions)
    // ------------------------------------------------------------------

    pub fn set_goals(&mut self, expected_version: u64, goals: NegGoals) -> Result<(), ApiError> {
        if self.closed {
            return Err(ApiError::new("session_closed", "session is closed", false));
        }
        if expected_version != self.goals_version {
            return Err(stale(&format!(
                "goals changed since this form was loaded (form v{expected_version}, current v{})",
                self.goals_version
            )));
        }
        self.goals = goals;
        self.goals_confirmed = true;
        self.goals_version += 1;
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    pub fn confirm_facts(
        &mut self,
        expected_state_rev: u64,
        offer_id: &str,
        items: &[(u64, u64)],
        now_ms: u64,
    ) -> Result<usize, ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("terms changed since this form was loaded"));
        }
        let Some(o) = self.active.as_mut() else {
            return Err(stale("no active offer"));
        };
        if o.id != offer_id {
            return Err(stale("offer was replaced"));
        }
        for (id, ver) in items {
            if !o.facts.iter().any(|f| f.id == *id && f.version == *ver) {
                return Err(stale("a term changed since this form was loaded"));
            }
        }
        let mut n = 0;
        for f in o.facts.iter_mut() {
            if items.iter().any(|(id, _)| *id == f.id) && !f.confirmed {
                f.confirmed = true;
                f.version += 1;
                n += 1;
            }
        }
        o.version += 1;
        self.record_human_wait(now_ms);
        self.bump_state();
        self.invalidate();
        Ok(n)
    }

    /// Explicit user correction of a negotiation value: replaces every statement
    /// in that slot (for price: every amount statement) with one confirmed
    /// user-entered fact and resolves open interpretations of that field.
    pub fn correct_value(
        &mut self,
        expected_state_rev: u64,
        offer_id: &str,
        c: Correction,
        now_ms: u64,
    ) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("terms changed since this form was loaded"));
        }
        let id = self.next_fact_id;
        let Some(o) = self.active.as_mut() else {
            return Err(stale("no active offer"));
        };
        if o.id != offer_id {
            return Err(stale("offer was replaced"));
        }
        let (stmt, field) = match c {
            Correction::TotalInclDelivery(cents) => (
                Stmt::Total {
                    cents,
                    covers: Covers::AllIn,
                },
                "amount",
            ),
            Correction::Size(v) if v.is_size() => (
                Stmt::Size {
                    negated: false,
                    value: v,
                },
                "size",
            ),
            Correction::Firmness(v) if v.is_firmness() => (
                Stmt::Firmness {
                    negated: false,
                    value: v,
                },
                "firmness",
            ),
            Correction::ReturnNights(n) => (
                Stmt::Return {
                    min_nights: n,
                    max_nights: n,
                },
                "returnperiod",
            ),
            _ => {
                return Err(ApiError::new(
                    "invalid_correction",
                    "value does not fit the field",
                    false,
                ))
            }
        };
        let is_price = field == "amount";
        o.facts.retain(|f| {
            let s = f.stmt.slot();
            if is_price {
                !matches!(
                    s,
                    Slot::Charge(_) | Slot::Total(_) | Slot::Unspecified | Slot::PriceUnresolved
                )
            } else {
                s != stmt.slot()
            }
        });
        o.facts.push(Fact {
            id,
            version: 1,
            stmt,
            confirmed: true,
            source_seq: None,
            excerpt: "user correction".into(),
            excerpt_truncated: false,
            byte_range: None,
            extractor: "user".into(),
        });
        for u in o
            .unresolved
            .iter_mut()
            .filter(|u| u.open && u.field == field)
        {
            u.open = false;
            u.version += 1;
        }
        o.version += 1;
        self.next_fact_id += 1;
        self.record_human_wait(now_ms);
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    /// Explicitly dismiss an open rejected interpretation ("not a term").
    pub fn dismiss_unresolved(
        &mut self,
        expected_state_rev: u64,
        offer_id: &str,
        id: u64,
        version: u64,
    ) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        let Some(o) = self.active.as_mut() else {
            return Err(stale("no active offer"));
        };
        if o.id != offer_id {
            return Err(stale("offer was replaced"));
        }
        let Some(u) = o
            .unresolved
            .iter_mut()
            .find(|u| u.id == id && u.version == version && u.open)
        else {
            return Err(stale("interpretation changed"));
        };
        u.open = false;
        u.version += 1;
        o.version += 1;
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    /// Resolve an unclear turn explicitly: apply its validated terms as a new
    /// offer or as an amendment of the active offer (unconfirmed), or ignore it.
    pub fn resolve_ambiguous(
        &mut self,
        expected_state_rev: u64,
        seq: u64,
        how: AmbiguousResolution,
    ) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        if !self.ambiguous.contains(&seq) {
            return Err(stale("turn is not marked unclear"));
        }
        let terms = self
            .turn(seq)
            .map(|t| t.pending_terms.clone())
            .unwrap_or_default();
        let mk = |id: u64, (stmt, ex, tr): &(Stmt, String, bool)| Fact {
            id,
            version: 1,
            stmt: stmt.clone(),
            confirmed: false,
            source_seq: Some(seq),
            excerpt: ex.clone(),
            excerpt_truncated: *tr,
            byte_range: None,
            extractor: "user-resolved".into(),
        };
        match how {
            AmbiguousResolution::Ignore => {}
            AmbiguousResolution::NewOffer => {
                if terms.is_empty() {
                    return Err(ApiError::new(
                        "no_terms",
                        "the unclear turn has no usable terms",
                        false,
                    ));
                }
                let id = self.offer_id(self.next_offer_no);
                self.next_offer_no += 1;
                let mut facts = Vec::new();
                for t in &terms {
                    facts.push(mk(self.next_fact_id, t));
                    self.next_fact_id += 1;
                }
                if let Some(old) = self.active.take() {
                    self.archive(old);
                }
                self.active = Some(Offer {
                    id,
                    version: 1,
                    facts,
                    conditions: vec![],
                    created_seq: seq,
                    turn_seqs: vec![seq],
                    resolved_through_seq: seq.saturating_sub(1),
                    dropped_through: 0,
                    unresolved: vec![],
                });
            }
            AmbiguousResolution::Amendment => {
                if terms.is_empty() {
                    return Err(ApiError::new(
                        "no_terms",
                        "the unclear turn has no usable terms",
                        false,
                    ));
                }
                let mut next = self.next_fact_id;
                let Some(o) = self.active.as_mut() else {
                    return Err(stale("no active offer to amend"));
                };
                let touched: BTreeSet<Slot> = terms.iter().map(|t| t.0.slot()).collect();
                let keep = o
                    .facts
                    .iter()
                    .filter(|f| !touched.contains(&f.stmt.slot()))
                    .count();
                if keep + terms.len() > FACTS_PER_OFFER {
                    return Err(ApiError::new(
                        "offer_full",
                        "offer fact limit reached",
                        false,
                    ));
                }
                o.facts.retain(|f| !touched.contains(&f.stmt.slot()));
                for t in &terms {
                    o.facts.push(mk(next, t));
                    next += 1;
                }
                o.version += 1;
                self.next_fact_id = next;
            }
        }
        self.ambiguous.remove(&seq);
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    /// Retry a failed user-triggered evaluation without changing facts.
    pub fn retry_evaluation(&mut self, expected_state_rev: u64) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        if self.eval_failed.is_none() {
            return Err(stale("no failed evaluation to retry"));
        }
        self.eval_failed = None;
        self.invalidate();
        Ok(())
    }

    pub fn resolve_condition(
        &mut self,
        expected_state_rev: u64,
        offer_id: &str,
        cond_id: u64,
        version: u64,
    ) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        let Some(o) = self.active.as_mut() else {
            return Err(stale("no active offer"));
        };
        if o.id != offer_id {
            return Err(stale("offer was replaced"));
        }
        let Some(c) = o
            .conditions
            .iter_mut()
            .find(|c| c.id == cond_id && c.version == version)
        else {
            return Err(stale("condition changed"));
        };
        if c.status != ConditionStatus::Unresolved {
            return Err(stale("condition already resolved"));
        }
        c.status = ConditionStatus::ResolvedByUser;
        c.version += 1;
        o.resolved_through_seq = o.resolved_through_seq.max(c.covers.1);
        o.version += 1;
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    pub fn discard_turn(&mut self, expected_state_rev: u64, seq: u64) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        let Some(t) = self.turn_mut(seq) else {
            return Err(stale("turn not found"));
        };
        if t.status != TurnStatus::FailedBeforeApply {
            return Err(stale(
                "only a turn that failed before its facts were applied can be discarded",
            ));
        }
        t.status = TurnStatus::Discarded;
        self.barriers.remove(&seq);
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    pub fn dismiss_ambiguous(&mut self, expected_state_rev: u64, seq: u64) -> Result<(), ApiError> {
        if expected_state_rev != self.state_rev {
            return Err(stale("state changed since this form was loaded"));
        }
        if !self.ambiguous.remove(&seq) {
            return Err(stale("turn is not marked unclear"));
        }
        self.bump_state();
        self.invalidate();
        Ok(())
    }

    /// Processing retry (PLAN v4 §4). Returns the ack result string.
    pub fn retry_turn(&mut self, seq: u64, attempt: u32) -> Result<&'static str, ApiError> {
        let later_ok = self.turns.iter().filter(|t| t.server_seq > seq).all(|t| {
            t.status.is_terminal() && matches!(t.effect.as_deref(), None | Some("no_change"))
        });
        let Some(t) = self.turn_mut(seq) else {
            return Err(ApiError::new("not_found", "turn not found", false));
        };
        if t.attempt != attempt {
            return Ok("attempt_mismatch");
        }
        match t.status {
            TurnStatus::FailedBeforeApply => {
                if !later_ok {
                    return Ok("retry_not_latest");
                }
                t.attempt += 1;
                t.attempts_total += 1;
                t.status = TurnStatus::Queued;
                t.error = None;
                self.queue.push_back(Job::Turn { seq });
                self.decision = None;
                self.phase = Phase::Processing;
                self.rev += 1;
                Ok("retry_started")
            }
            TurnStatus::FailedAfterApply => {
                t.attempt += 1;
                t.attempts_total += 1;
                t.status = TurnStatus::Applied;
                t.error = None;
                self.enqueue_eval(Some(seq));
                self.decision = None;
                self.phase = Phase::Processing;
                self.rev += 1;
                Ok("retry_started")
            }
            TurnStatus::Queued
            | TurnStatus::Extracting
            | TurnStatus::Applied
            | TurnStatus::Evaluating => Ok("retry_in_progress"),
            _ => Ok("not_failed"),
        }
    }

    pub fn cancel_turn(&mut self, seq: u64) -> &'static str {
        let Some(t) = self.turn_mut(seq) else {
            return "not_found";
        };
        match t.status {
            TurnStatus::Queued | TurnStatus::Extracting => {
                t.status = TurnStatus::Cancelled;
                self.invalidate();
                "cancelled"
            }
            TurnStatus::Cancelled => "cancelled",
            _ if t.effect_applied => "already_applied",
            _ => "not_cancellable",
        }
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.queue.clear();
        for t in self.turns.iter_mut() {
            if !t.status.is_terminal() {
                t.status = TurnStatus::Cancelled;
            }
        }
        self.decision = None;
        self.phase = Phase::Closed;
        self.rev += 1;
        self.bump_state();
    }

    pub fn active_offer_text(&self) -> String {
        match &self.active {
            None => "none".into(),
            Some(o) => {
                let cost = evaluate_cost(&o.stmts(), self.goals.budget_cents).1;
                format!(
                    "{} (lower bound {}, exact {})",
                    o.id,
                    fmt_cents(cost.lower_bound),
                    cost.exact
                        .map(fmt_cents)
                        .unwrap_or_else(|| "unknown".into())
                )
            }
        }
    }

    pub fn excerpt_of(text: &str) -> String {
        excerpt(text).0
    }
}

pub enum Rules {
    Idle,
    Final(Decision, Vec<(ConditionKind, (u64, u64), String)>),
    Contextual {
        base: Decision,
        request: ContextualRequest,
        covers: (u64, u64),
    },
}

/// User correction of one negotiation value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Correction {
    TotalInclDelivery(u64),
    Size(EnumValue),
    Firmness(EnumValue),
    ReturnNights(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmbiguousResolution {
    Ignore,
    NewOffer,
    Amendment,
}

fn rejected_lines(rejected: &[RejectedTerm]) -> Vec<String> {
    rejected
        .iter()
        .take(LIST_MAX_ENTRIES)
        .map(|r| bounded(&format!("{} ({})", r.reason, r.excerpt), 240).0)
        .collect()
}

/// Explicit wording that keeps the active offer's other terms.
pub const KEEP_TERMS_MARKERS: &[&str] = &[
    "same offer",
    "same deal",
    "keep all other terms",
    "keep the other terms",
    "keep everything else",
    "everything else stays",
    "everything else the same",
    "all other terms stay",
    "other terms stay",
    "rest stays the same",
    "otherwise the same",
];

pub fn keeps_other_terms(transcript: &str) -> bool {
    let n = crate::text::normalize(transcript);
    KEEP_TERMS_MARKERS.iter().any(|m| n.contains(m))
}
