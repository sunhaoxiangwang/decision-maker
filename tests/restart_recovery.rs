//! Restart recovery against a live mock copilot server (not run by default):
//!   COPILOT_ADDR=127.0.0.1:<port> COPILOT_BRIDGE_TOKEN=<token> \
//!     cargo test -p decision-maker --test restart_recovery -- --ignored
//! The server must run `copilot-server serve --mock` on that address with an empty data dir.
use decision_maker::copilot::Copilot;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;

fn http(method: &str, path: &str, extra: &str, body: &str) -> (u16, String) {
    let addr = std::env::var("COPILOT_ADDR").expect("COPILOT_ADDR");
    let mut s = TcpStream::connect(&addr).expect("copilot server");
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let status = raw.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, raw.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_string())
}

fn api(method: &str, path: &str, body: &str) -> Value {
    let token = std::env::var("COPILOT_BRIDGE_TOKEN").expect("COPILOT_BRIDGE_TOKEN");
    let auth = format!("Authorization: Bearer {token}\r\nContent-Type: application/json\r\n");
    serde_json::from_str(&http(method, path, &auth, body).1).unwrap()
}

/// A UI form post bound to the active session (as the browser sends it).
fn form(path: &str, fields: &str) -> u16 {
    let (_, page) = http("GET", "/", "", "");
    let csrf = page.split("name=\"csrf\" value=\"").nth(1).and_then(|r| r.split('"').next()).expect("csrf");
    let mut body = format!("csrf={csrf}&{fields}");
    let a = api("GET", "/v1/sessions/active", "");
    if let Some(sid) = a["session_id"].as_str() {
        body.push_str(&format!("&session_id={sid}&state_rev={}", a["state_rev"]));
    }
    http("POST", path, "Content-Type: application/x-www-form-urlencoded\r\n", &body).0
}

fn negotiation_session() -> String {
    assert_eq!(form("/ui/session", "mode=negotiation"), 303);
    assert_eq!(form("/ui/goals", "goals_version=1&size=queen&firmness=firm&budget=1200&nights=100"), 303);
    api("GET", "/v1/sessions/active", "")["session_id"].as_str().unwrap().to_string()
}

fn seq(r: &Value) -> u64 {
    r["turn"]["source_seq"].as_u64().or(r["update"]["source_seq"].as_u64()).expect("source_seq")
}

#[test]
#[ignore = "needs a running mock copilot server (COPILOT_ADDR, COPILOT_BRIDGE_TOKEN)"]
fn bridge_restart_and_session_change_recover() {
    let s1 = negotiation_session();
    let first = Copilot::from_env().expect("token");
    let mut last = 0;
    for t in 1..=3 {
        let r = first.submit(t, &format!("turn {t} from the first boot"), Some(0)).expect("first boot");
        assert_eq!(r["session_id"].as_str(), Some(s1.as_str()));
        assert!(seq(&r) > last);
        last = seq(&r);
    }

    // bridge restart: new boot, turn counter back at 1, same server and session
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = Copilot::from_env().expect("token");
    let r = second.submit(1, "turn 1 after the restart", Some(1)).expect("restart turn");
    assert!(seq(&r) > last, "{} <= {last}", seq(&r));
    last = seq(&r);
    let u = second.context_update(2, "our budget is now 1500 dollars").expect("restart context update");
    assert!(seq(&u) > last, "context update seq {} <= turn seq {last}", seq(&u));
    last = seq(&u);

    // a high-water mark far above the clock (e.g. a skewed clock): one stale reply, then resend
    let far = last + 1_000_000_000;
    let r = api("POST", &format!("/v1/sessions/{s1}/turns"), &serde_json::json!({
        "protocol": "vexo-copilot/1", "session_id": s1, "mode": "negotiation", "source_id": "cy-bridge",
        "client_turn_id": "other-boot-1", "source_seq": far,
        "transcript": {"text": "a turn from a skewed clock", "transcript_source": "simulated"}}).to_string());
    assert!(r["error"].is_null(), "{r}");
    let r = second.submit(3, "turn 3 after the skewed one", None).expect("stale recovery");
    assert_eq!(seq(&r), far + 1);

    // a new session in the UI: the bridge follows it without a restart
    let s2 = negotiation_session();
    assert_ne!(s1, s2);
    let r = second.submit(4, "turn 4 in the new session", None).expect("session change");
    assert_eq!(r["session_id"].as_str(), Some(s2.as_str()));
    assert!(seq(&r) > far + 1);

    // no active session: a clear error, no panic
    assert_eq!(form("/ui/close", ""), 303);
    let e = second.submit(5, "nobody is listening", None).unwrap_err();
    assert!(e.contains("no active session"), "{e}");
}
