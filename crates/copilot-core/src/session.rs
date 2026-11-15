//! Pure session state machine: turns, barriers, two-phase application and decision
//! publication. Negotiation turns are judged by the model against free-text goals;
//! lunch keeps its fact extraction and rules.
//! The server wraps a `Session` in a mutex and performs provider I/O outside it.

use crate::contract::*;
use crate::judge::Judgment;
use crate::limits::*;
use crate::lunch::{self, LunchState};
use crate::text::{bounded, excerpt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtractCtx {
    pub seq: u64,
    pub attempt: u32,
    pub mode: Mode,
    pub text: String,
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
    /// Negotiation: the model judges the conversation against the free-text goals.
    Judge {
        goals: String,
        conversation: String,
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
    Judge(Result<Judgment, ApiError>),
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
    /// Negotiation goals as free text, in the user's own words (may be empty).
    pub goals: String,
    pub goals_version: u64,
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
    /// Recent spoken goal updates (negotiation only), for duplicates and lookup.
    pub goal_updates: VecDeque<crate::context::GoalUpdate>,
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
            goals: String::new(),
            goals_version: 1,
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
            goal_updates: VecDeque::new(),
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
        // provenance and timings belong to the current generation: a spoken goal
        // update (timings unmeasured -> null) or the turn with that server_seq
        let gen_ctx = self
            .goal_updates
            .iter()
            .find(|u| u.server_seq == self.generation);
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
        // lunch facts from this turn are missing until it is retried or discarded; a
        // negotiation turn never blocks (the next judgement reads the whole conversation)
        if self.mode == Mode::Lunch {
            self.barriers.insert(seq);
        }
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

    fn mark_ambiguous(&mut self, seq: u64, why: &str) -> PhaseA {
        let Some(t) = self.turn_mut(seq) else {
            return PhaseA::Dropped;
        };
        t.status = TurnStatus::Applied;
        t.effect_applied = true;
        t.effect = Some("ambiguous".into());
        t.rejected_terms = vec![format!("unclear: {why}")];
        self.ambiguous.insert(seq);
        self.barriers.remove(&seq);
        self.bump_state();
        PhaseA::Applied
    }

    /// Negotiation Phase A: the turn simply joins the conversation the judge reads.
    pub fn apply_conversation(&mut self, seq: u64) -> bool {
        let closed = self.closed;
        let Some(t) = self.turn_mut(seq) else {
            return false;
        };
        if closed || t.status != TurnStatus::Extracting {
            return false;
        }
        t.status = TurnStatus::Applied;
        t.effect_applied = true;
        t.effect = Some("conversation".into());
        self.barriers.remove(&seq);
        self.bump_state();
        true
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
                return self.mark_ambiguous(seq, "options changed during extraction");
            }
            return PhaseA::Reextract;
        }
        if ext.turn_kind == lunch::LunchTurnKind::Ambiguous {
            return self.mark_ambiguous(seq, "unclear statement");
        }
        if !l.can_add(&valid) {
            return self.mark_ambiguous(seq, "option fact limit reached");
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
            Mode::Negotiation => self.judge_rules(trigger),
            Mode::Lunch => self.lunch_rules(memory),
        };
        match rules {
            Rules::Final(d) => {
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
            Rules::Contextual { base, request } => EvalStart::NeedsContextual(Box::new(EvalCtx {
                trigger,
                state_rev: self.state_rev,
                memory_version: memory.map(|m| m.publication).unwrap_or(0),
                request,
                base,
                deadline_ms,
            })),
        }
    }

    /// Commit Phase B atomically: validate the snapshot, then publish its decision.
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
        let mem_ok = memory.is_none_or(|m| {
            m.publication == ctx.memory_version
                && m.publication >= self.memory_seen
                && !m.pending_change
                && m.recovery_required.is_none()
        });
        if self.mode == Mode::Lunch && memory.is_none() {
            return Commit::Stale;
        }
        if ctx.state_rev != self.state_rev || !mem_ok {
            return Commit::Stale;
        }
        if let Some(seq) = ctx.trigger {
            if let Some(t) = self.turn_mut(seq) {
                t.timings.decide = Some(decide_ms);
            }
        }
        let mut d = ctx.base.clone();
        match result {
            ContextualResult::Judge(Ok(j)) => {
                let source = if self.providers == "real" {
                    "gemini_judge"
                } else {
                    "mock_judge"
                };
                d = Decision::simple(j.outcome, j.reason, source);
            }
            ContextualResult::Choice(Ok(a)) => {
                let l = self.lunch.as_ref();
                d = lunch::decide_with_choice(d, &a, l);
            }
            ContextualResult::Judge(Err(e)) | ContextualResult::Choice(Err(e)) => {
                d = Decision::simple(
                    Outcome::Unavailable,
                    format!("No decision: {}", e.message),
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
        self.publish(d, ctx.trigger, now_ms);
        Commit::Committed
    }

    // ------------------------------------------------------------------
    // Rules
    // ------------------------------------------------------------------

    /// The newest applied turns (up to the trigger) that fit the context window, oldest
    /// first, for the model to judge against the free-text goals.
    fn judge_rules(&self, trigger: Option<u64>) -> Rules {
        let upto = trigger.unwrap_or(u64::MAX);
        let mut lines: Vec<String> = Vec::new();
        let mut chars = 0;
        for t in self
            .turns
            .iter()
            .rev()
            .filter(|t| t.server_seq <= upto && t.effect_applied)
        {
            let line = format!("{}: {}", t.speaker.as_deref().unwrap_or("unknown"), t.text);
            chars += line.chars().count() + 1;
            if chars > CONTEXT_MAX_CHARS || lines.len() >= CONTEXT_MAX_TURNS {
                break;
            }
            lines.push(line);
        }
        if lines.is_empty() {
            return Rules::Idle;
        }
        lines.reverse();
        Rules::Contextual {
            base: Decision::simple(Outcome::None, "", "none"),
            request: ContextualRequest::Judge {
                goals: self.goals.clone(),
                conversation: lines.join("\n"),
            },
        }
    }

    fn lunch_rules(&self, memory: Option<&lunch::MemoryView>) -> Rules {
        let Some(l) = &self.lunch else {
            return Rules::Idle;
        };
        if !l.setup_confirmed {
            return Rules::Idle;
        }
        match lunch::rules(l, memory, &self.barriers, &self.ambiguous) {
            lunch::LunchRules::Final(d) => Rules::Final(d),
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
            },
        }
    }

    // ------------------------------------------------------------------
    // User actions (all bound to state_rev and object versions)
    // ------------------------------------------------------------------

    /// Replace the free-text goals (typed in the companion UI).
    pub fn set_goals(&mut self, expected_version: u64, goals: String) -> Result<(), ApiError> {
        if self.closed {
            return Err(ApiError::new("session_closed", "session is closed", false));
        }
        if expected_version != self.goals_version {
            return Err(stale(&format!(
                "goals changed since this form was loaded (form v{expected_version}, current v{})",
                self.goals_version
            )));
        }
        let goals = goals.trim().to_string();
        if goals.chars().count() > crate::context::GOALS_MAX_CHARS {
            return Err(ApiError::new(
                "goals_too_long",
                "goals exceed 4000 characters",
                false,
            ));
        }
        self.goals = goals;
        self.goals_version += 1;
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

    pub fn excerpt_of(text: &str) -> String {
        excerpt(text).0
    }
}

pub enum Rules {
    Idle,
    Final(Decision),
    Contextual {
        base: Decision,
        request: ContextualRequest,
    },
}
