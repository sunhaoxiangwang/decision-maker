//! Spoken goal updates (negotiation only). Words captured between three-wave gestures are
//! appended to the free-text goals as said and apply at once; the judge interprets them.

use crate::contract::{ApiError, Header, Mode};
use crate::limits::*;
use crate::session::{Accepted, Session};
use serde::{Deserialize, Serialize};

/// Goal updates kept for duplicate detection and lookup.
pub const CONTEXT_HISTORY: usize = 8;
/// Upper bound on the free-text goals.
pub const GOALS_MAX_CHARS: usize = 4_000;

pub const TRIGGER_THREE_WAVES: &str = "three_waves";
pub const TRIGGER_TYPED: &str = "typed_simulated";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CtxStatus {
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalUpdate {
    pub source_id: String,
    pub update_id: String,
    pub source_seq: u64,
    pub server_seq: u64,
    pub text: String,
    pub transcript_source: String,
    pub input_source: String,
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtxRef {
    pub source_id: String,
    pub update_id: String,
    pub source_seq: u64,
    pub server_seq: u64,
    pub attempt: u32,
}

/// Never actionable; `cue` is always null. `goals` is the full goals text after the update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUpdateResult {
    #[serde(flatten)]
    pub header: Header,
    pub update: CtxRef,
    pub status: CtxStatus,
    pub goals: String,
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

fn conflict(code: &str, msg: &str) -> ApiError {
    ApiError::new(code, msg, false)
}

impl Session {
    fn goal_update(&self, source_id: &str, update_id: &str) -> Option<&GoalUpdate> {
        self.goal_updates
            .iter()
            .find(|u| u.source_id == source_id && u.update_id == update_id)
    }

    /// Append spoken (or typed) words to the goals and re-judge the conversation.
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
        if let Some(u) = self.goal_update(&r.source_id, &r.update_id) {
            let same = r.mode == self.mode
                && r.source_seq.is_none_or(|s| s == u.source_seq)
                && u.text == r.text.trim()
                && u.transcript_source == r.transcript_source;
            return if same {
                Ok(Accepted::Duplicate(u.server_seq))
            } else {
                Err(conflict("update_id_reused", "update_id reused with different content"))
            };
        }
        if r.mode == Mode::Lunch || self.mode == Mode::Lunch {
            return Err(conflict(
                "context_update_unsupported",
                "voice context updates are supported for negotiation only",
            ));
        }
        if r.trigger != TRIGGER_THREE_WAVES && r.trigger != TRIGGER_TYPED {
            return Err(conflict("unsupported_trigger", "trigger must be three_waves"));
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
        let goals = if self.goals.is_empty() {
            text.clone()
        } else {
            format!("{}\n{text}", self.goals)
        };
        if goals.chars().count() > GOALS_MAX_CHARS {
            return Err(conflict(
                "goals_too_long",
                "goals exceed 4000 characters; shorten them in the copilot UI",
            ));
        }
        let seq = self.next_server_seq;
        self.next_server_seq += 1;
        self.source_last_seq.insert(r.source_id.clone(), source_seq);
        self.goal_updates.push_back(GoalUpdate {
            source_id: r.source_id,
            update_id: r.update_id,
            source_seq,
            server_seq: seq,
            text,
            transcript_source: r.transcript_source,
            input_source: r.input_source,
        });
        while self.goal_updates.len() > CONTEXT_HISTORY {
            self.goal_updates.pop_front();
        }
        self.goals = goals;
        self.goals_version += 1;
        // the re-judgement belongs to this update's generation, so older cues are ignored
        self.generation = seq;
        self.bump_state();
        self.invalidate();
        Ok(Accepted::New(seq))
    }

    pub fn context_result(
        &self,
        source_id: &str,
        update_id: &str,
        request_id: &str,
    ) -> Option<ContextUpdateResult> {
        let u = self.goal_update(source_id, update_id)?;
        Some(ContextUpdateResult {
            header: self.header(request_id, "context_update_result"),
            update: CtxRef {
                source_id: u.source_id.clone(),
                update_id: u.update_id.clone(),
                source_seq: u.source_seq,
                server_seq: u.server_seq,
                attempt: 1,
            },
            status: CtxStatus::Applied,
            goals: self.goals.clone(),
            input_source: u.input_source.clone(),
            transcript_source: u.transcript_source.clone(),
            cue: None,
            actionable: false,
        })
    }
}
