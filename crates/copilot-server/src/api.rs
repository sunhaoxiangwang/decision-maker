//! Bridge API `vexo-copilot/1` (PLAN v4 §9) and shared middleware.

use crate::app::{App, SessionHandle};
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use copilot_core::context::{ContextUpdateRequest, CtxAcceptReq, TRIGGER_THREE_WAVES};
use copilot_core::contract::*;
use copilot_core::limits::*;
use copilot_core::session::{AcceptReq, Accepted, Session};
use std::sync::Arc;
use std::time::Duration;

#[allow(clippy::too_many_arguments)] // one flat constructor for every error envelope field
fn err_env(
    app: &App,
    status: StatusCode,
    code: &str,
    msg: &str,
    retryable: bool,
    sid: Option<String>,
    mode: Option<Mode>,
    turn: Option<serde_json::Value>,
) -> Response {
    let env = ErrorEnvelope {
        header: Header {
            protocol: PROTOCOL.into(),
            request_id: app.request_id(),
            instance_id: app.instance_id.clone(),
            kind: "error".into(),
            session_id: sid,
            mode,
            rev: 0,
            state_rev: 0,
            generation: 0,
            error: Some(ApiError::new(code, msg, retryable)),
        },
        turn,
        cue: None,
        actionable: false,
    };
    (status, Json(env)).into_response()
}

fn status_for(code: &str) -> StatusCode {
    match code {
        "goals_unconfirmed" => StatusCode::PRECONDITION_FAILED,
        "empty_transcript" | "transcript_too_long" => StatusCode::UNPROCESSABLE_ENTITY,
        "turn_queue_full" => StatusCode::TOO_MANY_REQUESTS,
        "not_found" => StatusCode::NOT_FOUND,
        _ => StatusCode::CONFLICT,
    }
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Host allow-list on every route (DNS-rebinding protection).
pub async fn host_guard(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let p = crate::ui::port(&app.cfg.bind).to_string();
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if host != format!("127.0.0.1:{p}") && host != format!("localhost:{p}") {
        return (StatusCode::MISDIRECTED_REQUEST, "host not allowed").into_response();
    }
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "content-security-policy",
        "default-src 'none'; style-src 'self'; img-src 'self'; frame-src 'self'; form-action 'self'; frame-ancestors 'self'; base-uri 'none'"
            .parse()
            .expect("hdr"),
    );
    h.insert("cache-control", "no-store".parse().expect("hdr"));
    // same-origin, not no-referrer: with no-referrer browsers send `Origin: null` on form posts,
    // which check_post rejects. Other sites still receive no referrer.
    h.insert("referrer-policy", "same-origin".parse().expect("hdr"));
    resp
}

/// Global admission limit (64), including waiting polls.
pub async fn admission(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let Ok(_permit) = app.requests.clone().try_acquire_owned() else {
        return err_env(
            &app,
            StatusCode::SERVICE_UNAVAILABLE,
            "server_busy",
            "too many requests in flight",
            true,
            None,
            None,
            None,
        );
    };
    // bounded request lifetime (body reads included); waiting polls have their own cap
    let limit = if req.uri().path().ends_with("/cue") {
        POLL_WAIT_MAX_MS + 5_000
    } else {
        REQUEST_TIMEOUT_MS
    };
    match tokio::time::timeout(Duration::from_millis(limit), next.run(req)).await {
        Ok(r) => r,
        Err(_) => err_env(
            &app,
            StatusCode::SERVICE_UNAVAILABLE,
            "request_timeout",
            "request exceeded its time limit",
            true,
            None,
            None,
            None,
        ),
    }
}

/// Bearer token + JSON content type on bridge routes.
pub async fn bridge_auth(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let Some(tok) = app.cfg.bridge_token.as_deref() else {
        return err_env(
            &app,
            StatusCode::SERVICE_UNAVAILABLE,
            "bridge_disabled",
            "COPILOT_BRIDGE_TOKEN is not set",
            false,
            None,
            None,
            None,
        );
    };
    let got = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {tok}");
    if !ct_eq(got.as_bytes(), expected.as_bytes()) {
        return err_env(
            &app,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid bridge token",
            false,
            None,
            None,
            None,
        );
    }
    if req.method() == axum::http::Method::POST {
        let ct = req
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct.starts_with("application/json") {
            return err_env(
                &app,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                "content-type must be application/json",
                false,
                None,
                None,
                None,
            );
        }
    }
    next.run(req).await
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Wait until the turn reaches a reportable state or the timeout elapses.
pub async fn wait_turn(h: &SessionHandle, seq: u64, max_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(max_ms);
    loop {
        let notified = h.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let s = h.state.lock().expect("session");
            match s.turn(seq) {
                Some(t) if t.status.is_terminal() => return,
                None => return,
                _ => {}
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return;
        }
    }
}

pub async fn get_active(State(app): State<Arc<App>>) -> Response {
    match app.active() {
        Some(h) => {
            let s = h.state.lock().expect("session");
            Json(s.session_info(&app.request_id())).into_response()
        }
        None => err_env(
            &app,
            StatusCode::NOT_FOUND,
            "no_active_session",
            "no active session",
            true,
            None,
            None,
            None,
        ),
    }
}

/// The bridge gets the active session, or starts a negotiation session when there is none
/// (negotiation needs no setup: goals are optional free text).
pub async fn post_session(State(app): State<Arc<App>>) -> Response {
    let h = match app.active() {
        Some(h) => h,
        None => app.create_session(Mode::Negotiation),
    };
    let s = h.state.lock().expect("session");
    Json(s.session_info(&app.request_id())).into_response()
}

pub async fn post_turn(
    State(app): State<Arc<App>>,
    Path(sid): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    // malformed-request correlation: echo client_turn_id when parseable
    let loose: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
    let echo = loose
        .as_ref()
        .and_then(|v| v.get("client_turn_id").cloned())
        .map(|c| serde_json::json!({"client_turn_id": c}));
    let req: TurnRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_env(
                &app,
                StatusCode::BAD_REQUEST,
                "bad_request",
                &format!("invalid turn request: {e}"),
                false,
                Some(sid),
                None,
                echo,
            )
        }
    };
    if req.protocol != PROTOCOL {
        return err_env(
            &app,
            StatusCode::BAD_REQUEST,
            "unsupported_protocol",
            "protocol must be vexo-copilot/1",
            false,
            Some(sid),
            None,
            echo,
        );
    }
    if req.source_id != "cy-bridge" {
        return err_env(
            &app,
            StatusCode::FORBIDDEN,
            "source_mismatch",
            "bridge token belongs to source cy-bridge",
            false,
            Some(sid),
            None,
            echo,
        );
    }
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            echo,
        );
    };
    if req.session_id != sid {
        return err_env(
            &app,
            StatusCode::CONFLICT,
            "session_mismatch",
            "session_id differs from the path",
            false,
            Some(sid),
            None,
            echo,
        );
    }
    let accepted = {
        let mut s = h.state.lock().expect("session");
        let r = s.accept_turn(AcceptReq {
            mode: req.mode,
            source_id: req.source_id.clone(),
            client_turn_id: req.client_turn_id.clone(),
            source_seq: Some(req.source_seq),
            text: req.transcript.text.clone(),
            transcript_source: req.transcript.transcript_source.clone(),
            speaker: req.transcript.speaker.clone(),
            input_source: if req.transcript.transcript_source == "simulated" {
                "simulated".into()
            } else {
                "bridge".into()
            },
            now_ms: app.now_ms(),
        });
        h.publish(&s);
        r
    };
    let (seq, dup) = match accepted {
        Ok(Accepted::New(s)) => (s, false),
        Ok(Accepted::Duplicate(s)) => (s, true),
        Err(e) => {
            let st = status_for(&e.code);
            return err_env(
                &app,
                st,
                &e.code,
                &e.message,
                e.retryable,
                Some(sid),
                Some(h.mode),
                echo,
            );
        }
    };
    wait_turn(&h, seq, TURN_RESPONSE_WAIT_MS).await;
    let s = h.state.lock().expect("session");
    match s.turn_result(seq, &app.request_id()) {
        Some(mut tr) => {
            if dup {
                tr.header.kind = "turn_result".into();
            }
            let mut v = serde_json::to_value(&tr).unwrap_or_default();
            v["duplicate"] = serde_json::json!(dup);
            Json(v).into_response()
        }
        None => err_env(
            &app,
            StatusCode::CONFLICT,
            "stale_turn",
            "turn record no longer retained",
            false,
            Some(sid),
            Some(h.mode),
            echo,
        ),
    }
}

pub async fn get_turn(
    State(app): State<Arc<App>>,
    Path((sid, cid)): Path<(String, String)>,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    let s = h.state.lock().expect("session");
    match s
        .find_turn("cy-bridge", &cid)
        .and_then(|seq| s.turn_result(seq, &app.request_id()))
    {
        Some(mut tr) => {
            tr.snapshot = None;
            Json(tr).into_response()
        }
        None => err_env(
            &app,
            StatusCode::NOT_FOUND,
            "not_accepted",
            "no turn with that client_turn_id",
            false,
            Some(sid),
            Some(h.mode),
            None,
        ),
    }
}

fn ack(
    app: &App,
    s: &Session,
    turn: Option<TurnRef>,
    kind: &str,
    accepted: bool,
    result: &str,
) -> Response {
    Json(Ack {
        header: s.header(&app.request_id(), kind),
        turn,
        accepted,
        result: result.into(),
        cue: None,
        actionable: false,
        selected_option: None,
    })
    .into_response()
}

pub async fn post_cancel(
    State(app): State<Arc<App>>,
    Path((sid, cid)): Path<(String, String)>,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    let mut s = h.state.lock().expect("session");
    let Some(seq) = s.find_turn("cy-bridge", &cid) else {
        return ack(&app, &s, None, "cancel_ack", false, "not_found");
    };
    let r = s.cancel_turn(seq);
    h.publish(&s);
    let tref = s.turn(seq).map(Session::turn_ref);
    ack(&app, &s, tref, "cancel_ack", r == "cancelled", r)
}

#[derive(serde::Deserialize)]
pub struct RetryBody {
    pub attempt: u32,
}

pub async fn post_retry(
    State(app): State<Arc<App>>,
    Path((sid, cid)): Path<(String, String)>,
    Json(b): Json<RetryBody>,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    let mut s = h.state.lock().expect("session");
    let Some(seq) = s.find_turn("cy-bridge", &cid) else {
        return ack(&app, &s, None, "retry_ack", false, "not_found");
    };
    let r = s.retry_turn(seq, b.attempt).unwrap_or("not_found");
    h.publish(&s);
    let tref = s.turn(seq).map(Session::turn_ref);
    ack(&app, &s, tref, "retry_ack", r == "retry_started", r)
}

#[derive(serde::Deserialize)]
pub struct CueQ {
    pub after_rev: Option<u64>,
    pub wait_ms: Option<u64>,
}

pub async fn get_cue(
    State(app): State<Arc<App>>,
    Path(sid): Path<String>,
    Query(q): Query<CueQ>,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    let after = q.after_rev.unwrap_or(0);
    let wait = q.wait_ms.unwrap_or(0).min(POLL_WAIT_MAX_MS);
    let mut rx = h.cue_tx.subscribe();
    if rx.borrow().header.rev <= after && wait > 0 {
        let Ok(_p) = app.polls.clone().try_acquire_owned() else {
            return err_env(
                &app,
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_pollers",
                "too many waiting polls",
                true,
                Some(sid),
                Some(h.mode),
                None,
            );
        };
        let deadline = tokio::time::Instant::now() + Duration::from_millis(wait);
        while rx.borrow().header.rev <= after {
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                break;
            }
        }
    }
    let s = h.state.lock().expect("session");
    Json(s.snapshot(&app.request_id())).into_response()
}

pub async fn post_cue_report(
    State(app): State<Arc<App>>,
    Path(sid): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    if body.len() > 1024 {
        return err_env(
            &app,
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "cue report over 1 KiB",
            false,
            Some(sid),
            Some(h.mode),
            None,
        );
    }
    let report: CueReport = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err_env(
                &app,
                StatusCode::BAD_REQUEST,
                "bad_request",
                &format!("invalid cue report: {e}"),
                false,
                Some(sid),
                Some(h.mode),
                None,
            )
        }
    };
    let s = h.state.lock().expect("session");
    if let Err(why) = report.validate(s.rev) {
        return err_env(
            &app,
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_cue_report",
            why,
            false,
            Some(sid),
            Some(h.mode),
            None,
        );
    }
    tracing::info!(target: "cue_report", session = %sid, rev = report.rev, applied = %report.applied,
        submit_to_final_cue_applied_ms = report.bridge_durations_ms.get("submit_to_final_cue_applied_ms").copied().unwrap_or(0),
        "bridge-reported LED application (not hardware validation)");
    ack(
        &app,
        &s,
        None,
        "report_ack",
        true,
        "recorded_as_bridge_reported",
    )
}

pub async fn fallback(State(app): State<Arc<App>>, _req: Request<Body>) -> Response {
    err_env(
        &app,
        StatusCode::NOT_FOUND,
        "not_found",
        "no such route",
        false,
        None,
        None,
        None,
    )
}

pub fn _unused(_: HeaderMap) {}

/// Bridge latency report (durations on the bridge's own clock). Validated before it
/// is acknowledged; recorded as bridge-reported, never as hardware validation.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CueReport {
    pub protocol: String,
    pub rev: u64,
    pub applied: String,
    pub bridge_durations_ms: std::collections::BTreeMap<String, u64>,
}

const REPORT_KEYS: &[&str] = &[
    "mic_off_to_transcript_final",
    "submit_to_turn_response",
    "poll_receipt_to_led_call_return",
    "submit_to_final_cue_applied_ms",
];

impl CueReport {
    pub fn validate(&self, current_rev: u64) -> Result<(), &'static str> {
        if self.protocol != PROTOCOL {
            return Err("protocol must be vexo-copilot/1");
        }
        if self.rev == 0 || self.rev > current_rev {
            return Err("rev is not a published revision of this session");
        }
        if self.applied != "led_set" && self.applied != "led_off" {
            return Err("applied must be led_set or led_off");
        }
        if self.bridge_durations_ms.is_empty() || self.bridge_durations_ms.len() > REPORT_KEYS.len()
        {
            return Err("bridge_durations_ms must contain known duration keys");
        }
        for (k, v) in &self.bridge_durations_ms {
            if !REPORT_KEYS.contains(&k.as_str()) {
                return Err("unknown duration key");
            }
            if *v > CUE_REPORT_DURATION_MAX_MS {
                return Err("duration out of range");
            }
        }
        Ok(())
    }
}

/// Map framework rejections (body limit, JSON, query, path, method) on bridge
/// routes to the canonical JSON error envelope.
pub async fn canonical_errors(State(app): State<Arc<App>>, req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    let status = resp.status();
    let is_json = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json || !(status.is_client_error() || status.is_server_error()) {
        return resp;
    }
    let code = match status.as_u16() {
        400 => "bad_request",
        404 => "not_found",
        405 => "method_not_allowed",
        408 => "request_timeout",
        413 => "payload_too_large",
        415 => "unsupported_media_type",
        422 => "unprocessable",
        _ => "error",
    };
    err_env(
        &app,
        status,
        code,
        "request rejected",
        false,
        None,
        None,
        None,
    )
}

/// Occupancy counters for the retention stress check (bridge token required).
pub async fn get_occupancy(State(app): State<Arc<App>>) -> Response {
    let (active, closed) = {
        let r = app.registry.lock().expect("registry");
        (
            r.active.clone(),
            r.closed.iter().cloned().collect::<Vec<_>>(),
        )
    };
    let mut sessions = Vec::new();
    for h in active.iter().chain(closed.iter()) {
        let s = h.state.lock().expect("session");
        let rejected: usize = s.turns.iter().map(|t| t.rejected_terms.len()).sum();
        let max_turn_result = s
            .turns
            .iter()
            .filter_map(|t| s.turn_result(t.server_seq, "occupancy"))
            .map(|r| serde_json::to_vec(&r).map(|v| v.len()).unwrap_or(0))
            .max()
            .unwrap_or(0);
        sessions.push(serde_json::json!({
            "session_id": s.id, "closed": s.closed, "turns": s.turns.len(),
            "transcript_bytes": s.transcript_bytes, "goal_updates": s.goal_updates.len(),
            "barriers": s.barriers.len(), "ambiguous": s.ambiguous.len(), "rejected_terms": rejected,
            "lunch_unresolved": s.lunch.as_ref().map(|l| l.unresolved.len()).unwrap_or(0),
            "queue": s.queue.len(),
            "snapshot_bytes": serde_json::to_vec(&s.snapshot("occupancy")).map(|v| v.len()).unwrap_or(0),
            "max_turn_result_bytes": max_turn_result,
        }));
    }
    Json(serde_json::json!({
        "sessions_retained": sessions.len(),
        "requests_in_flight": MAX_INFLIGHT_REQUESTS - app.requests.available_permits(),
        "waiting_polls": MAX_WAITING_POLLS - app.polls.available_permits(),
        "jobs_running": app.jobs_running.load(std::sync::atomic::Ordering::SeqCst),
        "jobs_peak": app.jobs_peak.load(std::sync::atomic::Ordering::SeqCst),
        "limits": {"sessions": 1 + CLOSED_SESSIONS_KEPT, "turns": TURN_RECORDS_PER_SESSION,
                   "transcript_bytes": TRANSCRIPT_BYTES_PER_SESSION,
                   "goal_updates": copilot_core::context::CONTEXT_HISTORY,
                   "unresolved_turns": MAX_UNRESOLVED_TURNS, "lunch_unresolved": LUNCH_UNRESOLVED_MAX, "snapshot_bytes": SNAPSHOT_MAX_BYTES,
                   "turn_result_bytes": TURN_RESULT_MAX_BYTES},
        "sessions": sessions,
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Spoken goal updates (negotiation only; applied at once, never actionable)
// ---------------------------------------------------------------------------

fn ctx_status_for(code: &str) -> StatusCode {
    match code {
        "context_update_unsupported" | "unsupported_trigger" | "field_too_long" => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        c => status_for(c),
    }
}

pub async fn post_context_update(
    State(app): State<Arc<App>>,
    Path(sid): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let loose: Option<serde_json::Value> = serde_json::from_slice(&body).ok();
    let echo = loose
        .as_ref()
        .and_then(|v| v.get("update_id").cloned())
        .map(|c| serde_json::json!({"update_id": c}));
    let bad = |code: &str, st: StatusCode, msg: &str, sid: &str| {
        err_env(
            &app,
            st,
            code,
            msg,
            false,
            Some(sid.into()),
            None,
            echo.clone(),
        )
    };
    let req: ContextUpdateRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return bad(
                "bad_request",
                StatusCode::BAD_REQUEST,
                &format!("invalid context update request: {e}"),
                &sid,
            )
        }
    };
    if req.protocol != PROTOCOL {
        return bad(
            "unsupported_protocol",
            StatusCode::BAD_REQUEST,
            "protocol must be vexo-copilot/1",
            &sid,
        );
    }
    if req.source_id != "cy-bridge" {
        return bad(
            "source_mismatch",
            StatusCode::FORBIDDEN,
            "bridge token belongs to source cy-bridge",
            &sid,
        );
    }
    if req.trigger != TRIGGER_THREE_WAVES {
        return bad(
            "unsupported_trigger",
            StatusCode::UNPROCESSABLE_ENTITY,
            "trigger must be three_waves",
            &sid,
        );
    }
    let Some(h) = app.find(&sid) else {
        return bad(
            "session_not_found",
            StatusCode::NOT_FOUND,
            "unknown session",
            &sid,
        );
    };
    if req.session_id != sid {
        return bad(
            "session_mismatch",
            StatusCode::CONFLICT,
            "session_id differs from the path",
            &sid,
        );
    }
    let accepted = {
        let mut s = h.state.lock().expect("session");
        let r = s.accept_context(CtxAcceptReq {
            mode: req.mode,
            source_id: req.source_id.clone(),
            update_id: req.update_id.clone(),
            source_seq: Some(req.source_seq),
            trigger: req.trigger.clone(),
            text: req.transcript.text.clone(),
            transcript_source: req.transcript.transcript_source.clone(),
            input_source: if req.transcript.transcript_source == "simulated"
                || headers.contains_key("x-copilot-simulated")
            {
                "simulated".into()
            } else {
                "bridge".into()
            },
            now_ms: app.now_ms(),
        });
        h.publish(&s);
        r
    };
    let dup = match accepted {
        Ok(Accepted::New(_)) => false,
        Ok(Accepted::Duplicate(_)) => true,
        Err(e) => {
            return err_env(
                &app,
                ctx_status_for(&e.code),
                &e.code,
                &e.message,
                e.retryable,
                Some(sid),
                Some(h.mode),
                echo,
            )
        }
    };
    let s = h.state.lock().expect("session");
    match s.context_result(&req.source_id, &req.update_id, &app.request_id()) {
        Some(r) => {
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["duplicate"] = serde_json::json!(dup);
            Json(v).into_response()
        }
        None => err_env(
            &app,
            StatusCode::CONFLICT,
            "stale_update",
            "context update no longer retained",
            false,
            Some(sid),
            Some(h.mode),
            echo,
        ),
    }
}

pub async fn get_context_update(
    State(app): State<Arc<App>>,
    Path((sid, uid)): Path<(String, String)>,
) -> Response {
    let Some(h) = app.find(&sid) else {
        return err_env(
            &app,
            StatusCode::NOT_FOUND,
            "session_not_found",
            "unknown session",
            false,
            Some(sid),
            None,
            None,
        );
    };
    let s = h.state.lock().expect("session");
    match s.context_result("cy-bridge", &uid, &app.request_id()) {
        Some(r) => Json(r).into_response(),
        None => err_env(
            &app,
            StatusCode::NOT_FOUND,
            "not_accepted",
            "no context update with that update_id",
            false,
            Some(sid),
            Some(h.mode),
            None,
        ),
    }
}
