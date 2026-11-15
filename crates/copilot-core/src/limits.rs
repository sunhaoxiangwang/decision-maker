//! Resource limits (PLAN v3 §9, v4 §6). All byte limits are UTF-8 bytes.

pub const BRIDGE_BODY_MAX: usize = 16 * 1024;
pub const UI_FORM_MAX: usize = 8 * 1024;
pub const TRANSCRIPT_MAX_CHARS: usize = 4_000;
pub const MAX_INFLIGHT_REQUESTS: usize = 64;
pub const MAX_WAITING_POLLS: usize = 8;
pub const POLL_WAIT_MAX_MS: u64 = 20_000;
pub const TURN_RESPONSE_WAIT_MS: u64 = 15_000;

pub const CLOSED_SESSIONS_KEPT: usize = 4;
pub const TURN_RECORDS_PER_SESSION: usize = 64;
pub const TRANSCRIPT_BYTES_PER_SESSION: usize = 1024 * 1024;
pub const TURN_QUEUE_MAX: usize = 3;
/// Total queued jobs per session (turns, evaluations and the single coalesced
/// voice-context job). Obsolete context jobs are pruned, never counted twice.
pub const SESSION_JOB_QUEUE_MAX: usize = 16;
pub const OFFERS_PER_SESSION: usize = 20;
pub const TOMBSTONES_PER_SESSION: usize = 100;
pub const CONDITIONS_PER_OFFER: usize = 16;

pub const EXCERPT_MAX_BYTES: usize = 240;
pub const REASON_MAX_BYTES: usize = 1024;
pub const LIST_ENTRY_MAX_BYTES: usize = 64;
pub const LIST_MAX_ENTRIES: usize = 16;
pub const SNAPSHOT_MAX_BYTES: usize = 8 * 1024;
pub const TURN_RESULT_MAX_BYTES: usize = 12 * 1024;

pub const CONTEXT_MAX_TURNS: usize = 30;
pub const CONTEXT_MAX_CHARS: usize = 12_000;

pub const JOB_SLOTS: usize = 2;
pub const JOB_DEADLINE_MS: u64 = 14_000;
pub const QUEUE_WAIT_MAX_MS: u64 = 14_000;
pub const SLOT_WAIT_MS: u64 = 1_000;
pub const EXTRACT_TIMEOUT_MS: u64 = 10_000;
pub const DECIDE_TIMEOUT_MS: u64 = 2_000;
pub const CONNECT_TIMEOUT_MS: u64 = 2_000;

pub const EXTRACT_RESPONSE_MAX: usize = 64 * 1024;
pub const DECIDE_RESPONSE_MAX: usize = 32 * 1024;

pub const MEMORY_FILE_MAX: usize = 256 * 1024;
pub const MEMORY_PREFS_MAX: usize = 200;
pub const PREF_TEXT_MAX: usize = 280;
pub const CORRUPT_FILES_KEPT: usize = 3;

pub const CHOICE_SELECT_MIN: f64 = 0.60;
pub const PROB_TOLERANCE: f64 = 1e-3;

pub const CLIENT_TURN_ID_MAX: usize = 128;
pub const SOURCE_ID_MAX: usize = 64;
pub const LABEL_MAX: usize = 32;
/// Unresolved failed + unclear turns allowed before new turns are refused.
pub const MAX_UNRESOLVED_TURNS: usize = 16;
/// Server-side extraction cardinality (matches the schema's maxItems).
pub const MAX_EXTRACTED_TERMS: usize = 12;
pub const LUNCH_FACTS_PER_OPTION: usize = 24;
pub const LUNCH_UNRESOLVED_MAX: usize = 16;
pub const REQUEST_TIMEOUT_MS: u64 = 20_000;
pub const CUE_REPORT_DURATION_MAX_MS: u64 = 600_000;
pub const UNRESOLVED_PER_OFFER: usize = 16;
