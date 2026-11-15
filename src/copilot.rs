//! Client for Leo's copilot server (`vexo-copilot/1`, docs/contract-v1.md). The bridge only
//! submits final conversation turns; goals and memory are confirmed in the copilot UI.
use crate::bridge::Cue;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Copilot {
    addr: String,
    token: String,
    boot: u128,
}

impl Copilot {
    /// None unless COPILOT_BRIDGE_TOKEN is set (the same token the copilot server uses).
    pub fn from_env() -> Option<Copilot> {
        let token = std::env::var("COPILOT_BRIDGE_TOKEN").ok().filter(|t| !t.trim().is_empty())?;
        let addr = std::env::var("COPILOT_ADDR").unwrap_or_else(|_| "127.0.0.1:8787".into());
        let boot = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        Some(Copilot { addr, token, boot })
    }

    /// Submits one final turn and returns the `turn_result` (its `snapshot` is the cue, if any).
    pub fn submit(&self, turn_id: u64, text: &str, speaker: Option<u32>) -> Result<Value, String> {
        let active = self.request("GET", "/v1/sessions/active", None)?;
        let sid = active["session_id"].as_str().ok_or("no active copilot session: start one in the copilot UI")?;
        let body = json!({
            "protocol": "vexo-copilot/1", "session_id": sid, "mode": active["mode"],
            "source_id": "cy-bridge", "client_turn_id": format!("{}-{turn_id}", self.boot),
            "source_seq": turn_id,
            "transcript": {"text": text, "transcript_source": "deepgram",
                           "speaker": speaker.map_or("unknown".into(), |s| format!("speaker{s}"))},
        });
        self.request("POST", &format!("/v1/sessions/{sid}/turns"), Some(&body))
    }

    // ponytail: one blocking HTTP/1.1 request per call, loopback only; no keep-alive, no
    // cue long-poll yet (confirmation-wait cues arrive on the next turn), add when needed.
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let mut s = TcpStream::connect(&self.addr).map_err(|e| format!("copilot {}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
        let body = body.map(Value::to_string).unwrap_or_default();
        write!(s, "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.addr, self.token, body.len()).map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        s.take(256 * 1024).read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let raw = String::from_utf8_lossy(&raw);
        let (_, json) = raw.split_once("\r\n\r\n").ok_or("copilot: malformed response")?;
        let v: Value = serde_json::from_str(json).map_err(|e| format!("copilot: {e}"))?;
        match v["error"].as_object() {
            Some(e) => Err(format!("copilot: {}", e.get("code").and_then(Value::as_str).unwrap_or("error"))),
            None => Ok(v),
        }
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
}
