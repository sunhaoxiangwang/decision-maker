//! Voice-context updates for negotiation goals (PLAN v5, negotiation only).
//! A captured update is extracted into a *proposal* of absolute, grounded goal
//! values; nothing is applied until the user confirms that exact proposal.

use crate::contract::{ApiError, Header, Mode, Outcome, Phase};
use crate::limits::*;
use crate::money::fmt_cents;
use crate::negotiation::*;
use crate::session::{Accepted, Decision, Job, Session};
use serde::{Deserialize, Serialize};

/// Server-side cardinality for one update (the provider schema has no array bounds).
pub const MAX_CONTEXT_CHANGES: usize = 6;
/// Older updates retained for duplicate detection.
pub const CONTEXT_HISTORY: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CtxStatus {
    Pending,
    Proposed,
    Failed,
    Stale,
    Superseded,
    Applied,
    Discarded,
}

impl CtxStatus {
    /// Statuses that block positive recommendations.
    pub fn is_open(self) -> bool {
        matches!(
            self,
            CtxStatus::Pending | CtxStatus::Proposed | CtxStatus::Failed | CtxStatus::Stale
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CtxField {
    Budget,
    Size,
    Firmness,
    ReturnPeriod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum GoalValue {
    BudgetCents(u64),
    Size(EnumValue),
    Firmness(EnumValue),
    ReturnNights(u32),
}

impl GoalValue {
    pub fn field(&self) -> CtxField {
        match self {
            GoalValue::BudgetCents(_) => CtxField::Budget,
            GoalValue::Size(_) => CtxField::Size,
            GoalValue::Firmness(_) => CtxField::Firmness,
            GoalValue::ReturnNights(_) => CtxField::ReturnPeriod,
        }
    }
    pub fn describe(&self) -> String {
        match self {
            GoalValue::BudgetCents(c) => format!("budget {} incl. delivery", fmt_cents(*c)),
            GoalValue::Size(v) => format!("size {}", v.label()),
            GoalValue::Firmness(v) => format!("firmness {}", v.label()),
            GoalValue::ReturnNights(n) => format!("return >= {n} nights"),
        }
    }
    /// The current goal value for the same field.
    pub fn current(&self, g: &NegGoals) -> String {
        match self {
            GoalValue::BudgetCents(_) => GoalValue::BudgetCents(g.budget_cents).describe(),
            GoalValue::Size(_) => g
                .sizes
                .iter()
                .map(|v| format!("size {}", v.label()))
                .collect::<Vec<_>>()
                .join("/"),
            GoalValue::Firmness(_) => g
                .firmness
                .iter()
                .map(|v| format!("firmness {}", v.label()))
                .collect::<Vec<_>>()
                .join("/"),
            GoalValue::ReturnNights(_) => GoalValue::ReturnNights(g.min_return_nights).describe(),
        }
    }
    pub fn apply(&self, g: &mut NegGoals) {
        match self {
            GoalValue::BudgetCents(c) => g.budget_cents = *c,
            GoalValue::Size(v) => g.sizes = vec![*v],
            GoalValue::Firmness(v) => g.firmness = vec![*v],
            GoalValue::ReturnNights(n) => g.min_return_nights = *n,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalChange {
    pub value: GoalValue,
    pub old: String,
    pub new: String,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextUpdate {
    pub update_id: String,
    pub source_id: String,
    pub source_seq: u64,
    pub server_seq: u64,
    pub text: String,
    pub transcript_source: String,
    pub input_source: String,
    pub status: CtxStatus,
    pub attempt: u32,
    pub proposal_version: u64,
    /// Snapshot taken when extraction started (PLAN v5 correction 3).
    pub base_state_rev: u64,
    pub base_goals_version: u64,
    pub changes: Vec<GoalChange>,
    pub rejected: Vec<String>,
    pub error: Option<ApiError>,
    pub accepted_ms: u64,
}

// ---------------------------------------------------------------------------
// Provider contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCtxChange {
    pub field: CtxField,
    pub amount_text: Option<String>,
    pub enum_value: EnumValue,
    pub quantity_text: Option<String>,
    pub duration_unit: DurUnit,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CtxExtraction {
    pub changes: Vec<RawCtxChange>,
}

/// Schema sent to Gemini (no array bounds; cardinality enforced in Rust).
pub fn context_schema() -> serde_json::Value {
    serde_json::json!({
      "type": "object", "additionalProperties": false, "required": ["changes"],
      "properties": {"changes": {"type": "array", "items": {
        "type": "object", "additionalProperties": false,
        "required": ["field", "amount_text", "enum_value", "quantity_text", "duration_unit", "quote"],
        "properties": {
          "field": {"type": "string", "enum": ["budget", "size", "firmness", "return_period"]},
          "amount_text": {"type": ["string", "null"]},
          "enum_value": {"type": "string", "enum": ["twin","full","queen","king","cal_king","soft","medium","firm","extra_firm","not_applicable"]},
          "quantity_text": {"type": ["string", "null"]},
          "duration_unit": {"type": "string", "enum": ["nights","days","weeks","months","years","not_applicable"]},
          "quote": {"type": "string"}
        }}}}
    })
}

fn blank(field: TermField, quote: &str) -> RawTerm {
    RawTerm {
        field,
        amount_kind: AmountKindRaw::NotApplicable,
        extra_label: ExtraLabel::NotApplicable,
        total_covers: Covers::NotApplicable,
        relation: Relation::None,
        amount_text: None,
        currency: Currency::NotApplicable,
        predicate: Predicate::Is,
        enum_value: EnumValue::NotApplicable,
        quantity_text: None,
        duration_unit: DurUnit::NotApplicable,
        refers_to: RefersTo::ThisOffer,
        corrects_earlier_value_in_turn: false,
        quote: quote.into(),
    }
}

/// Validate one proposed change with the existing clause/grounding rules. Only
/// absolute values are accepted; amounts and durations are parsed in Rust.
pub fn validate_change(c: &RawCtxChange, transcript: &str) -> Result<GoalValue, String> {
    let r = |e: RejectedTerm| e.reason;
    match c.field {
        CtxField::Budget => {
            let raw = RawTerm {
                amount_kind: AmountKindRaw::Unspecified,
                amount_text: c.amount_text.clone(),
                currency: Currency::Usd,
                ..blank(TermField::Amount, &c.quote)
            };
            if relative_in_clause(&c.quote, transcript) {
                return Err("relative change; state the new total budget".into());
            }
            match validate_term(&raw, transcript).map_err(r)?.stmt {
                Stmt::Unspecified { cents } if cents > 0 && cents <= 100_000_000 => {
                    Ok(GoalValue::BudgetCents(cents))
                }
                _ => Err("budget must be one explicit USD amount".into()),
            }
        }
        CtxField::Size | CtxField::Firmness => {
            let tf = if c.field == CtxField::Size {
                TermField::Size
            } else {
                TermField::Firmness
            };
            let raw = RawTerm {
                enum_value: c.enum_value,
                ..blank(tf, &c.quote)
            };
            match validate_term(&raw, transcript).map_err(r)?.stmt {
                Stmt::Size {
                    negated: false,
                    value,
                } => Ok(GoalValue::Size(value)),
                Stmt::Firmness {
                    negated: false,
                    value,
                } => Ok(GoalValue::Firmness(value)),
                _ => Err("value must be stated affirmatively".into()),
            }
        }
        CtxField::ReturnPeriod => {
            let raw = RawTerm {
                quantity_text: c.quantity_text.clone(),
                duration_unit: c.duration_unit,
                ..blank(TermField::ReturnPeriod, &c.quote)
            };
            if relative_in_clause(&c.quote, transcript) {
                return Err("relative change; state the new return period".into());
            }
            match validate_term(&raw, transcript).map_err(r)?.stmt {
                Stmt::Return {
                    min_nights,
                    max_nights,
                } if min_nights == max_nights && min_nights <= 3650 => {
                    Ok(GoalValue::ReturnNights(min_nights))
                }
                Stmt::Return { .. } => {
                    Err("return period must be an exact number of nights, days or weeks".into())
                }
                _ => Err("unsupported return period".into()),
            }
        }
    }
}

/// Validate a whole extraction: (valid changes, rejected reasons). Two different
/// values for one field are a conflict and nothing for that field is proposed.
pub fn validate_extraction(
    ext: &CtxExtraction,
    transcript: &str,
    goals: &NegGoals,
) -> Result<(Vec<GoalChange>, Vec<String>), ApiError> {
    if ext.changes.len() > MAX_CONTEXT_CHANGES {
        return Err(ApiError::new(
            "extraction_invalid_output",
            format!(
                "{} changes exceed the {MAX_CONTEXT_CHANGES}-change contract",
                ext.changes.len()
            ),
            false,
        ));
    }
    let mut ok: Vec<GoalChange> = Vec::new();
    let mut rejected = Vec::new();
    // sticky for the whole extraction: once a field has two different readings,
    // no later reading (in any order or repetition) can reintroduce it
    let mut conflicted: Vec<CtxField> = Vec::new();
    for c in &ext.changes {
        match validate_change(c, transcript) {
            Ok(v) => {
                let f = v.field();
                if conflicted.contains(&f) {
                    continue;
                }
                if let Some(prev) = ok.iter().find(|g| g.value.field() == f) {
                    if prev.value != v {
                        rejected.push(format!("conflicting values for {f:?}").to_lowercase());
                        conflicted.push(f);
                        ok.retain(|g| g.value.field() != f);
                    }
                    continue;
                }
                ok.push(GoalChange {
                    old: v.current(goals),
                    new: v.describe(),
                    quote: crate::text::excerpt(&c.quote).0,
                    value: v,
                });
            }
            Err(e) => rejected.push(crate::text::bounded(&format!("{e} (\"{}\")", c.quote), 240).0),
        }
    }
    Ok((ok, rejected))
}

/// Complete-goal validation after replacing only the listed fields.
pub fn validate_goals(g: &NegGoals) -> Result<(), String> {
    if g.budget_cents == 0 || g.budget_cents > 100_000_000 {
        return Err("budget out of range".into());
    }
    if g.sizes.is_empty() || !g.sizes.iter().all(|v| v.is_size()) {
        return Err("invalid size".into());
    }
    if g.firmness.is_empty() || !g.firmness.iter().all(|v| v.is_firmness()) {
        return Err("invalid firmness".into());
    }
    if g.min_return_nights > 3650 {
        return Err("return period out of range".into());
    }
    Ok(())
}

/// Relative phrasing is never turned into an absolute goal ("$100 more").
const RELATIVE_MARKERS: &[&str] = &[
    "more",
    "less",
    "extra",
    "additional",
    "another",
    "plus",
    "minus",
    "by",
    "increase",
    "decrease",
    "bump",
    "on top",
    "fewer",
    "double",
    "half",
    "percent",
];

fn relative_in_clause(quote: &str, transcript: &str) -> bool {
    crate::text::support_clauses(quote, transcript)
        .iter()
        .any(|c| crate::text::has_any_phrase(&crate::text::tokens(c), RELATIVE_MARKERS))
}

// ---------------------------------------------------------------------------
// Bridge contract
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CtxTranscriptIn {
    pub text: String,
    pub transcript_source: String,
}

/// `POST /v1/sessions/{sid}/context-updates`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextUpdateRequest {
    pub protocol: String,
    pub session_id: String,
    pub mode: Mode,
    pub source_id: String,
    pub update_id: String,
    pub source_seq: u64,
    pub trigger: String,
    pub transcript: CtxTranscriptIn,
}

pub const TRIGGER_THREE_WAVES: &str = "three_waves";
pub const TRIGGER_TYPED: &str = "typed_simulated";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtxRef {
    pub source_id: String,
    pub update_id: String,
    pub source_seq: u64,
    pub server_seq: u64,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeView {
    pub field: CtxField,
    pub old: String,
    pub new: String,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalView {
    pub version: u64,
    /// Goals version the proposal was computed against (confirmation requires it to be current).
    pub base_goals_version: u64,
    /// Session `state_rev` recorded when extraction started (informational; the UI
    /// confirmation form is bound to the session's current `state_rev`).
    pub base_state_rev: u64,
    pub changes: Vec<ChangeView>,
    pub rejected: Vec<String>,
}

/// Never actionable; `cue` is always null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUpdateResult {
    #[serde(flatten)]
    pub header: Header,
    pub update: CtxRef,
    pub status: CtxStatus,
    pub proposal: Option<ProposalView>,
    pub input_source: String,
    pub transcript_source: String,
    pub cue: Option<serde_json::Value>,
    pub actionable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtxAcceptReq {
    pub mode: Mode,
    pub source_id: String,
    pub update_id: String,
    pub source_seq: Option<u64>,
    pub trigger: String,
    pub text: String,
    pub transcript_source: String,
    pub input_source: String,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CtxJob {
    pub seq: u64,
    pub attempt: u32,
    pub text: String,
    pub goals_summary: String,
    pub base_state_rev: u64,
    pub base_goals_version: u64,
    pub deadline_ms: u64,
}

fn conflict(code: &str, msg: &str) -> ApiError {
    ApiError::new(code, msg, false)
}

pub fn goals_summary(g: &NegGoals) -> String {
    format!(
        "{}; {}; {}; {}",
        GoalValue::BudgetCents(g.budget_cents).current(g),
        GoalValue::Size(EnumValue::Queen).current(g),
        GoalValue::Firmness(EnumValue::Firm).current(g),
        GoalValue::ReturnNights(g.min_return_nights).current(g)
    )
}

impl Session {
    /// An update that blocks positive recommendations.
    pub fn context_open(&self) -> bool {
        self.context.as_ref().is_some_and(|c| c.status.is_open())
    }

    /// The retained context update that opened generation `seq`, if any.
    pub fn context_by_seq(&self, seq: u64) -> Option<&ContextUpdate> {
        self.context
            .iter()
            .chain(self.context_history.iter())
            .find(|c| c.server_seq == seq)
    }

    fn context_lookup(&self, source_id: &str, update_id: &str) -> Option<&ContextUpdate> {
        self.context
            .iter()
            .chain(self.context_history.iter())
            .find(|c| c.source_id == source_id && c.update_id == update_id)
    }

    /// Gate used by the negotiation rules (after PAUSE and the turn barrier).
    pub fn context_gate(&self) -> Option<Decision> {
        let c = self.context.as_ref().filter(|c| c.status.is_open())?;
        let what = match c.status {
            CtxStatus::Pending => "is being processed",
            CtxStatus::Proposed => "awaits your confirmation",
            CtxStatus::Failed => "failed; retry or discard it",
            _ => "is stale; retry or discard it",
        };
        let mut d = Decision::simple(
            Outcome::Ask,
            format!("Ask: voice context update {} {what}.", c.update_id),
            "rules",
        );
        d.missing = vec![format!("voice context update {what}")];
        Some(d)
    }

    fn context_transition(&mut self) {
        self.bump_state();
        self.invalidate();
    }

    pub fn accept_context(&mut self, r: CtxAcceptReq) -> Result<Accepted, ApiError> {
        if self.closed {
            return Err(conflict("session_closed", "session is closed"));
        }
        if r.update_id.is_empty()
            || r.update_id.len() > CLIENT_TURN_ID_MAX
            || r.source_id.len() > SOURCE_ID_MAX
            || r.transcript_source.len() > LABEL_MAX
            || r.trigger.len() > LABEL_MAX
        {
            return Err(conflict(
                "field_too_long",
                "update_id, source_id, transcript_source or trigger exceeds its limit",
            ));
        }
        if let Some(c) = self.context_lookup(&r.source_id, &r.update_id) {
            let same = r.mode == self.mode
                && r.source_seq.is_none_or(|s| s == c.source_seq)
                && c.text == r.text.trim()
                && c.transcript_source == r.transcript_source;
            return if same {
                Ok(Accepted::Duplicate(c.server_seq))
            } else {
                Err(conflict(
                    "update_id_reused",
                    "update_id reused with different content",
                ))
            };
        }
        if r.mode == Mode::Lunch || self.mode == Mode::Lunch {
            return Err(conflict(
                "context_update_unsupported",
                "voice context updates are supported for negotiation only",
            ));
        }
        if r.trigger != TRIGGER_THREE_WAVES && r.trigger != TRIGGER_TYPED {
            return Err(conflict(
                "unsupported_trigger",
                "trigger must be three_waves",
            ));
        }
        let last = self.source_last_seq.get(&r.source_id).copied().unwrap_or(0);
        let source_seq = match r.source_seq {
            Some(s) if s <= last => {
                return Err(conflict(
                    "stale_update",
                    &format!("source_seq {s} <= last accepted {last} for {}", r.source_id),
                ))
            }
            Some(s) => s,
            None => last + 1,
        };
        let text = r.text.trim().to_string();
        if text.is_empty() {
            return Err(conflict("empty_transcript", "transcript is empty"));
        }
        if text.chars().count() > TRANSCRIPT_MAX_CHARS {
            return Err(conflict(
                "transcript_too_long",
                "transcript exceeds 4000 characters",
            ));
        }
        if !self.goals_confirmed && !self.auto_goals {
            return Err(conflict(
                "goals_unconfirmed",
                "confirm goals before updating them by voice",
            ));
        }
        let others = self
            .queue
            .iter()
            .filter(|j| !matches!(j, Job::Context { .. }))
            .count();
        if others >= SESSION_JOB_QUEUE_MAX {
            return Err(ApiError::new(
                "turn_queue_full",
                "too many jobs waiting; retry shortly",
                true,
            ));
        }
        let seq = self.next_server_seq;
        self.next_server_seq += 1;
        self.source_last_seq.insert(r.source_id.clone(), source_seq);
        if let Some(mut old) = self.context.take() {
            if old.status.is_open() {
                old.status = CtxStatus::Superseded;
            }
            self.context_history.push_back(old);
            while self.context_history.len() > CONTEXT_HISTORY {
                self.context_history.pop_front();
            }
        }
        self.context = Some(ContextUpdate {
            update_id: r.update_id,
            source_id: r.source_id,
            source_seq,
            server_seq: seq,
            text,
            transcript_source: r.transcript_source,
            input_source: r.input_source,
            status: CtxStatus::Pending,
            attempt: 1,
            proposal_version: 0,
            base_state_rev: 0,
            base_goals_version: 0,
            changes: vec![],
            rejected: vec![],
            error: None,
            accepted_ms: r.now_ms,
        });
        self.generation = seq;
        self.bump_state();
        self.decision = None;
        self.phase = Phase::Processing;
        self.rev += 1;
        // only the current update can run: coalesce obsolete queued context jobs
        self.queue.retain(|j| !matches!(j, Job::Context { .. }));
        self.queue.push_back(Job::Context { seq });
        Ok(Accepted::New(seq))
    }

    /// Snapshot the base revisions and the transcript before calling the provider.
    pub fn start_context(&mut self, seq: u64, now_ms: u64, deadline_ms: u64) -> Option<CtxJob> {
        if self.closed {
            return None;
        }
        let (state_rev, goals_version, summary) = (
            self.state_rev,
            self.goals_version,
            goals_summary(&self.goals),
        );
        let c = self
            .context
            .as_mut()
            .filter(|c| c.server_seq == seq && c.status == CtxStatus::Pending)?;
        if now_ms.saturating_sub(c.accepted_ms) > QUEUE_WAIT_MAX_MS {
            c.status = CtxStatus::Failed;
            c.error = Some(ApiError::new(
                "queue_timeout",
                "context update waited too long",
                true,
            ));
            self.context_transition();
            return None;
        }
        c.base_state_rev = state_rev;
        c.base_goals_version = goals_version;
        Some(CtxJob {
            seq,
            attempt: c.attempt,
            text: c.text.clone(),
            goals_summary: summary,
            base_state_rev: state_rev,
            base_goals_version: goals_version,
            deadline_ms,
        })
    }

    /// Commit the provider result as a proposal (never applied here).
    pub fn commit_context(
        &mut self,
        job: &CtxJob,
        res: Result<CtxExtraction, ApiError>,
        now_ms: u64,
    ) {
        if self.closed {
            return;
        }
        let (state_rev, goals_version) = (self.state_rev, self.goals_version);
        let goals = self.goals.clone();
        let next_proposal = self.context_proposals + 1;
        let Some(c) = self.context.as_mut().filter(|c| {
            c.server_seq == job.seq && c.attempt == job.attempt && c.status == CtxStatus::Pending
        }) else {
            return; // superseded, discarded or retried meanwhile
        };
        let mut proposed = false;
        if now_ms > job.deadline_ms {
            c.status = CtxStatus::Failed;
            c.error = Some(ApiError::new(
                "job_deadline",
                "context update exceeded its deadline",
                true,
            ));
        } else if state_rev != job.base_state_rev || goals_version != job.base_goals_version {
            c.status = CtxStatus::Stale;
            c.error = Some(ApiError::new(
                "context_stale",
                "goals or state changed during extraction",
                true,
            ));
        } else {
            match res.and_then(|ext| validate_extraction(&ext, &c.text, &goals)) {
                Err(e) => {
                    c.status = CtxStatus::Failed;
                    c.error = Some(e);
                }
                Ok((changes, rejected)) => {
                    c.rejected = rejected;
                    if changes.is_empty() {
                        c.status = CtxStatus::Failed;
                        c.error = Some(ApiError::new(
                            "context_no_change",
                            "no explicit, absolute goal value was found",
                            false,
                        ));
                    } else {
                        c.changes = changes;
                        c.proposal_version = next_proposal;
                        c.status = CtxStatus::Proposed;
                        proposed = true;
                    }
                }
            }
        }
        if proposed {
            self.context_proposals = next_proposal;
            if self.auto_goals {
                self.auto_apply_context();
            }
        }
        self.context_transition();
    }

    /// Auto mode: apply the proposal just made, as if the user had confirmed it. While goals
    /// are unset, the stated fields replace the placeholders; goals count as set (and turns
    /// are decided) once size, firmness, budget and return period have all been stated.
    fn auto_apply_context(&mut self) {
        let c = self.context.as_ref().expect("current").clone();
        let mut g = self.goals.clone();
        for ch in &c.changes {
            ch.value.apply(&mut g);
        }
        if let Err(e) = validate_goals(&g) {
            let cur = self.context.as_mut().expect("current");
            cur.status = CtxStatus::Failed;
            cur.error = Some(ApiError::new("context_invalid_goals", e, false));
            return;
        }
        self.goals = g;
        if !self.goals_confirmed {
            for ch in &c.changes {
                if !self.stated_goals.contains(&ch.value.field()) {
                    self.stated_goals.push(ch.value.field());
                }
            }
            self.goals_confirmed = self.stated_goals.len() == 4;
        }
        self.goals_version += 1;
        let missing: Vec<&str> = [
            (CtxField::Size, "size"),
            (CtxField::Firmness, "firmness"),
            (CtxField::Budget, "budget"),
            (CtxField::ReturnPeriod, "return_period"),
        ]
        .into_iter()
        .filter(|(f, _)| !self.goals_confirmed && !self.stated_goals.contains(f))
        .map(|(_, name)| name)
        .collect();
        let cur = self.context.as_mut().expect("current");
        cur.status = CtxStatus::Applied;
        cur.error = None;
        if !missing.is_empty() {
            cur.rejected.push(format!("still needed before decisions: {}", missing.join(", ")));
        }
    }

    /// Worker-side failure (capacity, deadline) for the current attempt.
    pub fn fail_context(&mut self, seq: u64, err: ApiError) {
        if let Some(c) = self
            .context
            .as_mut()
            .filter(|c| c.server_seq == seq && c.status == CtxStatus::Pending)
        {
            c.status = CtxStatus::Failed;
            c.error = Some(err);
            self.context_transition();
        }
    }

    fn context_current(&self, update_id: &str) -> Result<&ContextUpdate, ApiError> {
        if let Some(c) = self.context.as_ref().filter(|c| c.update_id == update_id) {
            return Ok(c);
        }
        match self
            .context_history
            .iter()
            .find(|c| c.update_id == update_id)
        {
            Some(c) if c.status == CtxStatus::Applied => Err(conflict(
                "context_already_applied",
                "this update was already applied",
            )),
            Some(_) => Err(conflict(
                "context_superseded",
                "a newer context update replaced this one",
            )),
            None => Err(conflict("not_found", "unknown context update")),
        }
    }

    pub fn confirm_context(
        &mut self,
        expected_state_rev: u64,
        update_id: &str,
        proposal_version: u64,
    ) -> Result<(), ApiError> {
        if self.closed {
            return Err(conflict("session_closed", "session is closed"));
        }
        if self.mode != Mode::Negotiation {
            return Err(conflict("context_update_unsupported", "negotiation only"));
        }
        let c = self.context_current(update_id)?.clone();
        match c.status {
            CtxStatus::Proposed => {}
            CtxStatus::Applied => {
                return Err(conflict(
                    "context_already_applied",
                    "this update was already applied",
                ))
            }
            _ => return Err(stale_ctx("this update is not awaiting confirmation")),
        }
        if c.proposal_version != proposal_version || expected_state_rev != self.state_rev {
            return Err(stale_ctx("the proposal changed since this form was loaded"));
        }
        if !self.goals_confirmed {
            return Err(conflict("goals_unconfirmed", "confirm goals first"));
        }
        if c.base_goals_version != self.goals_version {
            let c = self.context.as_mut().expect("current");
            c.status = CtxStatus::Stale;
            c.error = Some(ApiError::new(
                "context_stale",
                "goals changed after this proposal",
                true,
            ));
            self.context_transition();
            return Err(stale_ctx(
                "goals changed after this proposal; retry the update",
            ));
        }
        let mut g = self.goals.clone();
        for ch in &c.changes {
            ch.value.apply(&mut g);
        }
        validate_goals(&g).map_err(|e| stale_ctx(&e))?;
        self.goals = g;
        self.goals_version += 1;
        let c = self.context.as_mut().expect("current");
        c.status = CtxStatus::Applied;
        c.error = None;
        self.context_transition();
        Ok(())
    }

    pub fn discard_context(
        &mut self,
        expected_state_rev: u64,
        update_id: &str,
    ) -> Result<(), ApiError> {
        if self.closed {
            return Err(conflict("session_closed", "session is closed"));
        }
        let c = self.context_current(update_id)?.clone();
        if c.status == CtxStatus::Applied {
            return Err(conflict(
                "context_already_applied",
                "an applied update cannot be discarded (discard never undoes goals)",
            ));
        }
        if !c.status.is_open() {
            return Err(stale_ctx("this update is already closed"));
        }
        if expected_state_rev != self.state_rev {
            return Err(stale_ctx("state changed since this form was loaded"));
        }
        let c = self.context.as_mut().expect("current");
        c.status = CtxStatus::Discarded;
        self.queue.retain(|j| !matches!(j, Job::Context { .. }));
        self.context_transition();
        Ok(())
    }

    pub fn retry_context(
        &mut self,
        expected_state_rev: u64,
        update_id: &str,
        now_ms: u64,
    ) -> Result<(), ApiError> {
        if self.closed {
            return Err(conflict("session_closed", "session is closed"));
        }
        let c = self.context_current(update_id)?.clone();
        if !matches!(
            c.status,
            CtxStatus::Failed | CtxStatus::Stale | CtxStatus::Proposed
        ) {
            return Err(stale_ctx(
                "only a failed, stale or proposed update can be retried",
            ));
        }
        if expected_state_rev != self.state_rev {
            return Err(stale_ctx("state changed since this form was loaded"));
        }
        let c = self.context.as_mut().expect("current");
        c.status = CtxStatus::Pending;
        c.attempt += 1;
        c.changes.clear();
        c.rejected.clear();
        c.error = None;
        c.proposal_version = 0;
        c.accepted_ms = now_ms;
        let seq = c.server_seq;
        self.bump_state();
        self.decision = None;
        self.phase = Phase::Processing;
        self.rev += 1;
        // only the current update can run: coalesce obsolete queued context jobs
        self.queue.retain(|j| !matches!(j, Job::Context { .. }));
        self.queue.push_back(Job::Context { seq });
        Ok(())
    }

    pub fn context_result(
        &self,
        source_id: &str,
        update_id: &str,
        request_id: &str,
    ) -> Option<ContextUpdateResult> {
        let c = self.context_lookup(source_id, update_id)?;
        let mut header = self.header(request_id, "context_update_result");
        header.error = c.error.clone();
        Some(ContextUpdateResult {
            header,
            update: CtxRef {
                source_id: c.source_id.clone(),
                update_id: c.update_id.clone(),
                source_seq: c.source_seq,
                server_seq: c.server_seq,
                attempt: c.attempt,
            },
            status: c.status,
            proposal: (c.proposal_version > 0).then(|| ProposalView {
                version: c.proposal_version,
                base_goals_version: c.base_goals_version,
                base_state_rev: c.base_state_rev,
                changes: c
                    .changes
                    .iter()
                    .map(|g| ChangeView {
                        field: g.value.field(),
                        old: g.old.clone(),
                        new: g.new.clone(),
                        quote: g.quote.clone(),
                    })
                    .collect(),
                rejected: c.rejected.clone(),
            }),
            input_source: c.input_source.clone(),
            transcript_source: c.transcript_source.clone(),
            cue: None,
            actionable: false,
        })
    }
}

fn stale_ctx(msg: &str) -> ApiError {
    ApiError::new("stale_form", msg, false)
}
