//! Continuous cue polling (docs/contract-v1.md "Bridge requirements"). A background thread
//! long-polls `GET /v1/sessions/{sid}/cue` (one waiting poll at a time) so a confirmation in
//! the copilot UI reaches the band without another spoken turn. Every cue, polled or from a
//! turn result, passes the same `copilot_core::bridge_model::BridgeModel`: instance, session,
//! mode, generation and revision are checked before anything is shown.
use crate::bridge::CueHandle;
use crate::copilot::{self, Copilot};
use copilot_core::bridge_model::{Action, BridgeModel};
use copilot_core::contract::{CueKind, CueSnapshot, Mode};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One model shared by the poller and the transcript loop.
pub type Model = Arc<Mutex<BridgeModel>>;

const WAIT_MS: u64 = 15_000; // under the 20 s read timeout of Copilot::request
const RETRY: Duration = Duration::from_secs(1);
const BLOCKED_RETRY: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Show this cue (`hex` None never happens here: clear cues become `LedOff`).
    Apply { rev: u64, hex: String, pattern: String },
    /// LED off (clear cue / processing / invalidated / closed, new session, capture, disconnect).
    LedOff(&'static str),
    Ignore(&'static str),
}

pub fn new_model() -> Model {
    Arc::new(Mutex::new(BridgeModel::new("bridge")))
}

fn ids(v: &Value) -> Option<(String, String, Mode)> {
    let mode = serde_json::from_value::<Mode>(v["mode"].clone()).ok()?;
    Some((v["instance_id"].as_str()?.to_string(), v["session_id"].as_str()?.to_string(), mode))
}

/// `session_info` from `GET /v1/sessions/active`. Ok((sid, true)) = new instance/session/mode:
/// the model was reset (last_rev, min_generation, awaiting) and the LED must go off.
pub fn on_active(m: &mut BridgeModel, info: &Value) -> Result<(String, bool), &'static str> {
    let (instance, session, mode) = ids(info).ok_or("malformed session_info")?;
    let changed = m.on_session(&instance, &session, mode).contains(&Action::LedOff);
    Ok((session, changed))
}

/// A `cue_snapshot` (polled, or embedded in a turn result).
pub fn on_cue(m: &mut BridgeModel, snap: &Value) -> Step {
    let Ok(s) = serde_json::from_value::<CueSnapshot>(snap.clone()) else {
        return Step::Ignore("malformed snapshot");
    };
    let same = m.instance.as_deref() == Some(s.header.instance_id.as_str())
        && m.session.as_deref() == s.header.session_id.as_deref()
        && m.mode.is_some()
        && m.mode == s.header.mode;
    if !same {
        // we only poll the model's session: a mismatch means it changed under us
        return Step::LedOff("instance/session/mode mismatch");
    }
    match m.on_snapshot(&s) {
        Action::Apply { rev } => match s.cue {
            Some(c) if c.kind != CueKind::Clear => Step::Apply { rev, hex: c.rgb.to_ascii_lowercase(), pattern: c.pattern },
            _ => Step::LedOff("clear cue"),
        },
        Action::Ignore(why) => Step::Ignore(why),
        _ => Step::Ignore("unexpected model action"),
    }
}

/// A final conversation transcript is being submitted: new capture epoch. No cue applies
/// until this epoch's turn response arrives. Returns the epoch's client_turn_id.
pub fn capture(m: &mut BridgeModel) -> String {
    let (id, _led_off) = m.capture_start();
    m.mic_stopped(); // the turn's speech is complete (Deepgram final)
    id
}

/// The synchronous response to this epoch's submission (`turn_result`).
pub fn on_turn_result(m: &mut BridgeModel, client_turn_id: &str, result: &Value) -> Step {
    let Some((instance, session, mode)) = ids(result) else {
        return Step::Ignore("malformed turn_result");
    };
    let changed = m.on_session(&instance, &session, mode).contains(&Action::LedOff);
    m.on_turn_response(&instance, &session, mode, client_turn_id, result["turn"]["server_seq"].as_u64());
    match &result["snapshot"] {
        Value::Null if changed => Step::LedOff("new instance/session/mode"),
        Value::Null => Step::Ignore("no snapshot"),
        snap => on_cue(m, snap),
    }
}

/// A voice context update takes the next generation: older cues must not apply after it.
pub fn on_context_result(m: &mut BridgeModel, result: &Value) {
    if let (Some((instance, session, mode)), Some(seq)) = (ids(result), result["update"]["server_seq"].as_u64()) {
        if m.instance.as_deref() == Some(instance.as_str()) && m.session.as_deref() == Some(session.as_str()) && m.mode == Some(mode) {
            m.min_generation = m.min_generation.max(seq);
        }
    }
}

/// Backend unreachable or a poll error: fail closed.
pub fn on_disconnect() -> Step {
    Step::LedOff("disconnected")
}

/// Still the newest applied cue for the same session with no capture or submission pending.
fn still_current(model: &Model, session: &str, rev: u64) -> bool {
    let m = model.lock().unwrap();
    m.session.as_deref() == Some(session) && m.last_rev == rev && !m.mic_active && m.awaiting.is_none() && !m.blocked
}

fn clear(cues: &CueHandle, why: &str) {
    match cues.clear_if_lit() {
        Ok(true) => println!("  LED off ({why})"),
        Ok(false) => (),
        Err(e) => eprintln!("  LED off ({why}) failed: {e}"),
    }
}

/// Shows an applied step; reports decision receipt and LED application separately.
pub fn apply(cues: &CueHandle, model: &Model, session: &str, step: &Step, snap: &Value) {
    match step {
        Step::Apply { rev, hex, pattern } => {
            println!("  decision received rev {rev} ({} {})", snap["outcome"], snap["phase"]);
            if let Some(ev) = copilot::decision_event(0, snap, now_ms()) {
                let _ = cues.publish(&ev);
            }
            let (m, s, r) = (model.clone(), session.to_string(), *rev);
            let shown = cues.show_snapshot_cue(Some(hex), pattern, move || still_current(&m, &s, r));
            let ok = matches!(shown, Ok(true));
            println!("  LED applied rev {rev}: {ok}{}", shown.err().map(|e| format!(" ({e})")).unwrap_or_default());
            let _ = cues.publish(&format!(
                r#"{{"type":"cue","turn_id":0,"rev":{rev},"cue":"{hex} {pattern}","shown":{ok},"time_ms":{}}}"#,
                now_ms()
            ));
        }
        Step::LedOff(why) => clear(cues, why),
        Step::Ignore(_) => (),
    }
}

/// Starts the background poller. `copilot` is the poller's own client.
pub fn spawn(copilot: Copilot, model: Model, cues: CueHandle) {
    std::thread::spawn(move || {
        let mut seen = 0u64; // newest rev the poller has looked at in this session
        loop {
            let info = match copilot.active_session() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("  cue poll: {e}");
                    apply(&cues, &model, "", &on_disconnect(), &Value::Null);
                    std::thread::sleep(RETRY);
                    continue;
                }
            };
            let step = on_active(&mut model.lock().unwrap(), &info);
            let sid = match step {
                Ok((sid, changed)) => {
                    if changed {
                        seen = 0;
                        println!("  copilot session {sid} ({}): LED off, cue state reset", info["mode"]);
                        clear(&cues, "new instance/session/mode");
                    }
                    sid
                }
                Err(e) => {
                    eprintln!("  cue poll: {e}");
                    std::thread::sleep(RETRY);
                    continue;
                }
            };
            let after = seen.max(model.lock().unwrap().last_rev);
            match copilot.cue(&sid, after, WAIT_MS) {
                Ok(snap) => {
                    let rev = snap["rev"].as_u64().unwrap_or(0);
                    let step = on_cue(&mut model.lock().unwrap(), &snap);
                    match step {
                        // transient: look again shortly (the turn response may never carry it)
                        Step::Ignore("capturing" | "awaiting submission acknowledgement") => std::thread::sleep(BLOCKED_RETRY),
                        _ => seen = seen.max(rev),
                    }
                    apply(&cues, &model, &sid, &step, &snap);
                }
                Err(e) => {
                    eprintln!("  cue poll: {e}");
                    apply(&cues, &model, &sid, &on_disconnect(), &Value::Null);
                    std::thread::sleep(RETRY);
                }
            }
        }
    });
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info(instance: &str, session: &str) -> Value {
        json!({"protocol": "vexo-copilot/1", "request_id": "r", "instance_id": instance, "kind": "session_info",
               "session_id": session, "mode": "negotiation", "rev": 1, "state_rev": 1, "generation": 0, "error": null})
    }

    fn snap(instance: &str, session: &str, rev: u64, generation: u64, phase: &str, cue: Value) -> Value {
        json!({"protocol": "vexo-copilot/1", "request_id": "r", "instance_id": instance, "kind": "cue_snapshot",
               "session_id": session, "mode": "negotiation", "rev": rev, "state_rev": 1, "generation": generation,
               "error": null, "current": true, "phase": phase, "outcome": "PAUSE", "selected_option": null,
               "cue": cue, "actionable": true, "reason": null, "missing": [], "conflicts": [], "unconfirmed": [],
               "conditions": [], "barriers": [], "truncated": false,
               "provenance": {"input_source": "ui", "transcript_source": null, "providers": "mock", "pipeline": "rust", "decision_source": "rules"},
               "timings_ms": {"accept_to_publish": null, "extract": null, "evaluate": null, "decide": null}})
    }

    fn red() -> Value {
        json!({"kind": "decision", "name": "red", "rgb": "#FF0000", "pattern": "blink", "period_ms": 400})
    }

    fn ready() -> BridgeModel {
        let mut m = BridgeModel::new("b");
        assert_eq!(on_active(&mut m, &info("i1", "s1")), Ok(("s1".into(), true)));
        assert_eq!(on_active(&mut m, &info("i1", "s1")), Ok(("s1".into(), false)));
        m
    }

    fn red_apply(rev: u64) -> Step {
        Step::Apply { rev, hex: "#ff0000".into(), pattern: "blink".into() }
    }

    #[test]
    fn applies_newer_rev_once_and_ignores_stale() {
        let mut m = ready();
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 5, 0, "decided", red())), red_apply(5));
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 5, 0, "decided", red())), Step::Ignore("revision not newer"));
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 4, 0, "decided", red())), Step::Ignore("revision not newer"));
        assert_eq!(on_cue(&mut m, &json!({"kind": "cue_snapshot"})), Step::Ignore("malformed snapshot"));
    }

    #[test]
    fn other_session_or_instance_is_ignored_and_clears() {
        let mut m = ready();
        assert_eq!(on_cue(&mut m, &snap("i1", "s2", 9, 0, "decided", red())), Step::LedOff("instance/session/mode mismatch"));
        assert_eq!(on_cue(&mut m, &snap("i2", "s1", 9, 0, "decided", red())), Step::LedOff("instance/session/mode mismatch"));
        assert_eq!(m.last_rev, 0);
        // restart: new instance from /v1/sessions/active resets the model (LED off)
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 7, 0, "decided", red())), red_apply(7));
        assert_eq!(on_active(&mut m, &info("i2", "s1")), Ok(("s1".into(), true)));
        assert_eq!(m.last_rev, 0);
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 8, 0, "decided", red())), Step::LedOff("instance/session/mode mismatch"));
        assert_eq!(on_cue(&mut m, &snap("i2", "s1", 2, 0, "decided", red())), red_apply(2));
    }

    #[test]
    fn generation_below_min_is_ignored() {
        let mut m = ready();
        let id = capture(&mut m);
        let turn = |seq: u64, snapshot: Value| json!({"instance_id": "i1", "session_id": "s1", "mode": "negotiation",
            "kind": "turn_result", "turn": {"client_turn_id": "x", "server_seq": seq}, "snapshot": snapshot});
        assert_eq!(on_turn_result(&mut m, &id, &turn(4, Value::Null)), Step::Ignore("no snapshot"));
        assert_eq!(m.min_generation, 4);
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 10, 3, "decided", red())), Step::Ignore("older generation"));
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 11, 4, "decided", red())), red_apply(11));
        // a voice context update takes the next generation too
        on_context_result(&mut m, &json!({"instance_id": "i1", "session_id": "s1", "mode": "negotiation", "update": {"server_seq": 5}}));
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 12, 4, "decided", red())), Step::Ignore("older generation"));
    }

    #[test]
    fn capture_in_progress_blocks_cues_until_this_epochs_response() {
        let mut m = ready();
        let id = capture(&mut m);
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 3, 0, "decided", red())), Step::Ignore("awaiting submission acknowledgement"));
        m.mic_active = true;
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 3, 0, "decided", red())), Step::Ignore("capturing"));
        m.mic_stopped();
        // a response for another submission does not unblock
        let r = |cid: &str| json!({"instance_id": "i1", "session_id": "s1", "mode": "negotiation",
            "turn": {"client_turn_id": cid, "server_seq": 1}, "snapshot": snap("i1", "s1", 4, 1, "decided", red())});
        assert_eq!(on_turn_result(&mut m, "b-999", &r("b-999")), Step::Ignore("awaiting submission acknowledgement"));
        assert_eq!(on_turn_result(&mut m, &id, &r(&id)), red_apply(4));
        // the poller later sees the same rev: not applied twice
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 4, 1, "decided", red())), Step::Ignore("revision not newer"));
    }

    #[test]
    fn invalid_state_and_disconnect_clear() {
        let mut m = ready();
        let clear = json!({"kind": "clear", "name": "off", "rgb": "#000000", "pattern": "solid", "period_ms": null});
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 2, 0, "processing", clear.clone())), Step::LedOff("clear cue"));
        assert_eq!(on_cue(&mut m, &snap("i1", "s1", 3, 0, "invalidated", clear)), Step::LedOff("clear cue"));
        assert_eq!(m.last_rev, 3);
        let mut idle = snap("i1", "s1", 4, 0, "idle", Value::Null);
        idle["actionable"] = json!(false);
        assert_eq!(on_cue(&mut m, &idle), Step::Ignore("not actionable"));
        assert_eq!(on_disconnect(), Step::LedOff("disconnected"));
    }
}
