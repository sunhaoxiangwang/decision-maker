//! Client for Leo's copilot server (`vexo-copilot/1`, docs/contract-v1.md). The bridge only
//! submits final conversation turns; goals and memory are confirmed in the copilot UI.
use crate::bridge::Cue;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Copilot {
    addr: String,
    token: String,
    boot: u128,
    /// Last `source_seq` sent; one counter shared by turns and context updates.
    seq: AtomicU64,
    /// The session the previous submission went to, so a change is logged, never silent.
    session: Mutex<Option<Active>>,
}

/// The active session as `GET /v1/sessions/active` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub instance_id: String,
    pub session_id: String,
    pub mode: Value,
    pub latest_generation: u64,
}

impl Active {
    fn from_info(v: &Value) -> Option<Active> {
        Some(Active {
            instance_id: v["instance_id"].as_str()?.to_string(),
            session_id: v["session_id"].as_str()?.to_string(),
            mode: v["mode"].clone(),
            latest_generation: v["latest_generation"].as_u64().unwrap_or(0),
        })
    }
    fn same_session(&self, o: &Active) -> bool {
        (&self.instance_id, &self.session_id, &self.mode) == (&o.instance_id, &o.session_id, &o.mode)
    }
}

/// `source_seq` slots per wall-clock millisecond. Every seq is at least `now_ms * SEQ_PER_MS`,
/// so a restarted bridge (turn counter back at 1) starts above every seq an earlier boot sent,
/// as long as that boot sent under 1000 per ms and the clock did not go back; a `stale_*`
/// reply covers the rest. 2026 epoch ms * 1000 is about 1.8e15 < 2^53: exact in JSON doubles.
const SEQ_PER_MS: u64 = 1000;

/// The next `source_seq`: above the last one sent, the wall-clock floor and the server's mark.
pub fn next_seq(last: u64, now_ms: u64, server_last: u64) -> u64 {
    (last + 1).max(now_ms.saturating_mul(SEQ_PER_MS)).max(server_last.saturating_add(1))
}

/// The server's high-water mark from a `stale_turn`/`stale_update` message
/// ("source_seq S <= last accepted N for cy-bridge").
pub fn stale_mark(message: &str) -> Option<u64> {
    message.split("last accepted ").nth(1)?.split_whitespace().next()?.parse().ok()
}

/// Rejections a re-discovery plus one resend can fix (restart, session or instance change).
const RECOVERABLE: [&str; 6] =
    ["stale_turn", "stale_update", "session_mismatch", "session_not_found", "session_closed", "mode_mismatch"];

impl Copilot {
    /// None unless COPILOT_BRIDGE_TOKEN is set (the same token the copilot server uses).
    pub fn from_env() -> Option<Copilot> {
        let token = std::env::var("COPILOT_BRIDGE_TOKEN").ok().filter(|t| !t.trim().is_empty())?;
        let addr = std::env::var("COPILOT_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
        let boot = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        Some(Copilot { addr, token, boot, seq: AtomicU64::new(0), session: Mutex::new(None) })
    }

    /// Submits one final turn and returns the `turn_result` (its `snapshot` is the cue, if any).
    pub fn submit(&self, turn_id: u64, text: &str, speaker: Option<u32>) -> Result<Value, String> {
        let speaker = speaker.map_or("unknown".into(), |s| format!("speaker{s}"));
        self.post("turn", |a, seq| {
            let sid = &a.session_id;
            (format!("/v1/sessions/{sid}/turns"), json!({
                "protocol": "vexo-copilot/1", "session_id": sid, "mode": a.mode,
                "source_id": "cy-bridge", "client_turn_id": format!("{}-{turn_id}", self.boot),
                "source_seq": seq,
                "transcript": {"text": text, "transcript_source": "deepgram", "speaker": speaker},
            }))
        })
    }

    /// Submits one final transcript captured between waves as a voice context update. The
    /// result is never actionable: the proposal is confirmed (or discarded) in the copilot UI.
    /// `source_seq` shares the turn counter, so turns and updates stay strictly ordered.
    pub fn context_update(&self, turn_id: u64, text: &str) -> Result<Value, String> {
        self.post("context update", |a, seq| {
            let sid = &a.session_id;
            (format!("/v1/sessions/{sid}/context-updates"), json!({
                "protocol": "vexo-copilot/1", "session_id": sid, "mode": a.mode,
                "source_id": "cy-bridge", "update_id": format!("{}-ctx-{turn_id}", self.boot),
                "source_seq": seq, "trigger": "three_waves",
                "transcript": {"text": text, "transcript_source": "deepgram"},
            }))
        })
    }

    /// Discovers the active session, posts `build(session, source_seq)`, and on a stale seq or
    /// a session/instance change re-discovers and resends once (logged) with a higher seq.
    fn post(&self, what: &str, build: impl Fn(&Active, u64) -> (String, Value)) -> Result<Value, String> {
        let mut active = self.discover()?;
        let mut mark = 0;
        for resend in [false, true] {
            let seq = self.take_seq(mark);
            let (path, body) = build(&active, seq);
            let v = self.call("POST", &path, Some(&body))?;
            let Some(err) = v["error"].as_object() else { return Ok(v) };
            let code = err.get("code").and_then(Value::as_str).unwrap_or("error");
            if resend || !RECOVERABLE.contains(&code) {
                return Err(format!("copilot: {code}"));
            }
            let stale = code.starts_with("stale_");
            if stale {
                mark = err.get("message").and_then(Value::as_str).and_then(stale_mark).unwrap_or(0);
            }
            let next = self.discover()?;
            if !stale && next.same_session(&active) {
                return Err(format!("copilot: {code}"));
            }
            eprintln!("  copilot: {what} rejected ({code}); resending once to session {} (instance {})",
                next.session_id, next.instance_id);
            active = next;
        }
        unreachable!("the loop returns on the resend")
    }

    /// `GET /v1/sessions/active`; logs when the instance, session or mode differs from the last.
    fn discover(&self) -> Result<Active, String> {
        let v = self.request("GET", "/v1/sessions/active", None).map_err(|e| match e.as_str() {
            "copilot: no_active_session" => "copilot: no active session: start one in the copilot UI".into(),
            _ => e,
        })?;
        let a = Active::from_info(&v).ok_or("copilot: malformed session_info")?;
        let mut last = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = last.as_ref().filter(|p| !p.same_session(&a)) {
            eprintln!("  copilot: active session changed {} {} ({}) -> {} {} ({})",
                p.instance_id, p.session_id, p.mode, a.instance_id, a.session_id, a.mode);
        }
        *last = Some(a.clone());
        Ok(a)
    }

    /// Reserves the next `source_seq` (strictly increasing within this boot).
    fn take_seq(&self, server_last: u64) -> u64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let step = |last| next_seq(last, now, server_last);
        step(self.seq.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |l| Some(step(l))).unwrap_or_else(|l| l))
    }

    // ponytail: one blocking HTTP/1.1 request per call, loopback only; no keep-alive, no
    // cue long-poll yet (confirmation-wait cues arrive on the next turn), add when needed.
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let v = self.call(method, path, body)?;
        match v["error"].as_object() {
            Some(e) => Err(format!("copilot: {}", e.get("code").and_then(Value::as_str).unwrap_or("error"))),
            None => Ok(v),
        }
    }

    /// `request` without the error mapping: an error envelope comes back as Ok.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let mut s = TcpStream::connect(&self.addr).map_err(|e| format!("copilot {}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
        let body = body.map(Value::to_string).unwrap_or_default();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.addr, self.token, body.len()).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.take(256 * 1024).read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let raw = String::from_utf8_lossy(&raw);
        let (_, json) = raw.split_once("\r\n\r\n").ok_or("copilot: malformed response")?;
        serde_json::from_str(json).map_err(|e| format!("copilot: {e}"))
    }
}

/// The LED cue for an actionable snapshot (Leo's outcome table). None = do not apply.
pub fn cue_of(snapshot: &Value) -> Option<Cue> {
    if snapshot["kind"] != "cue_snapshot" || snapshot["actionable"] != true {
        return None;
    }
    Some(match snapshot["outcome"].as_str()? {
        "PROCEED" => Cue::Proceed,
        "PAUSE" => Cue::Pause,
        "ASK" => Cue::Ask,
        "SELECT" => Cue::Lunch(snapshot["selected_option"].as_str()?.chars().next()?),
        _ => Cue::Off,
    })
}

/// The snapshot as the UI's `decision` event (web/lib/store.ts DecisionEvent).
pub fn decision_event(turn_id: u64, snapshot: &Value, time_ms: u128) -> Option<String> {
    let cue = match cue_of(snapshot)? {
        Cue::Proceed => "Proceed".to_string(),
        Cue::Pause => "Pause".into(),
        Cue::Ask => "Ask".into(),
        Cue::Lunch(c) => format!("Lunch('{c}')"),
        Cue::Off => "Off".into(),
    };
    let strs = |k: &str| snapshot[k].as_array().cloned().unwrap_or_default();
    Some(json!({
        "type": "decision", "id": format!("rev-{}", snapshot["rev"]), "turn_id": turn_id,
        "mode": snapshot["mode"], "cue": cue, "reason": snapshot["reason"],
        "facts": [], "missing": strs("missing"), "conflicts": strs("conflicts"), "memory_used": [],
        "latency_ms": {"transcript_to_decision": snapshot["timings_ms"]["accept_to_publish"]},
        "time_ms": time_ms as u64,
    }).to_string())
}

// Cue polling (src/cue_poll.rs): session discovery and one waiting cue poll at a time.
impl Copilot {
    /// `session_info` for the active session (instance_id, session_id, mode, rev, ...).
    pub fn active_session(&self) -> Result<Value, String> {
        self.request("GET", "/v1/sessions/active", None)
    }

    /// The session's `cue_snapshot`, waiting up to `wait_ms` (server caps it at 20000) for
    /// `rev > after_rev`. Keep `wait_ms` under the 20 s read timeout of `request`.
    pub fn cue(&self, sid: &str, after_rev: u64, wait_ms: u64) -> Result<Value, String> {
        if sid.is_empty() || !sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err("copilot: unexpected session id".into());
        }
        self.request("GET", &format!("/v1/sessions/{sid}/cue?after_rev={after_rev}&wait_ms={wait_ms}"), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_actionable_snapshots_become_cues() {
        let snap = |outcome: &str, actionable| json!({"kind": "cue_snapshot", "actionable": actionable,
            "outcome": outcome, "selected_option": "B", "rev": 3, "mode": "lunch", "reason": "r",
            "missing": ["x"], "conflicts": [], "timings_ms": {"accept_to_publish": 12}});
        assert_eq!(cue_of(&snap("PAUSE", true)), Some(Cue::Pause));
        assert_eq!(cue_of(&snap("SELECT", true)), Some(Cue::Lunch('B')));
        assert_eq!(cue_of(&snap("UNAVAILABLE", true)), Some(Cue::Off));
        assert_eq!(cue_of(&snap("PROCEED", false)), None);
        assert_eq!(cue_of(&json!({"kind": "turn_result", "actionable": true, "outcome": "PROCEED"})), None);
        let ev: Value = serde_json::from_str(&decision_event(7, &snap("SELECT", true), 1).unwrap()).unwrap();
        assert_eq!((ev["cue"].as_str(), ev["turn_id"].as_u64(), ev["id"].as_str()), (Some("Lunch('B')"), Some(7), Some("rev-3")));
    }

    #[test]
    fn seqs_survive_a_restart_and_stay_json_safe() {
        let boot1 = 1_794_000_000_000; // ms, Nov 2026
        let mut last = 0;
        for i in 0..5_000 {
            last = next_seq(last, boot1 + i / 10, 0); // 10 submissions per ms, well above speech
        }
        // a restart 50 ms later: the turn counter is back at 1, the seq is not
        assert!(next_seq(0, boot1 + 550, 0) > last);
        // strictly increasing within a boot, even if the clock stalls or goes back
        assert_eq!(next_seq(last, boot1, 0), last + 1);
        // the server's high-water mark always wins
        assert_eq!(next_seq(5, 1, u64::MAX / 2), u64::MAX / 2 + 1);
        assert!(next_seq(0, 7_000_000_000_000, 0) < 1 << 53); // year 2191
    }

    #[test]
    fn stale_mark_reads_the_server_message() {
        assert_eq!(stale_mark("source_seq 3 <= last accepted 1794000000000123 for cy-bridge"), Some(1794000000000123));
        assert_eq!(stale_mark("session is closed"), None);
    }
}
