//! In-process HTTP tests against a real listener on 127.0.0.1:0.

use copilot_core::lunch::{Allergen, PrefKind, Strength};
use copilot_providers::mock::{MockDecider, MockExtractor};
use copilot_providers::{Decider, Extractor, Unconfigured};
use copilot_server::app::App;
use copilot_server::memory::{Change, FailPoint, SaveResult};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct T {
    app: Arc<App>,
    addr: String,
    http: reqwest::Client,
    _dir: TempDir,
}

struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir(tag: &str) -> TempDir {
    let p = std::env::temp_dir().join(format!(
        "copilot-test-{tag}-{}-{}",
        std::process::id(),
        uuid_like()
    ));
    std::fs::create_dir_all(&p).unwrap();
    TempDir(p)
}
fn uuid_like() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

async fn start_with(
    ex: Arc<dyn Extractor>,
    de: Arc<dyn Decider>,
    label: &str,
    fail: Option<FailPoint>,
    gate: Option<copilot_server::memory::Gate>,
    dir: TempDir,
) -> T {
    let (app, addr) =
        copilot_server::spawn(ex, de, label, dir.0.clone(), Some("tok".into()), fail, gate).await;
    T {
        app,
        addr,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        _dir: dir,
    }
}
async fn start(label: &str) -> T {
    let (ex, de): (Arc<dyn Extractor>, Arc<dyn Decider>) = if label == "mock" {
        (Arc::new(MockExtractor), Arc::new(MockDecider))
    } else {
        (Arc::new(Unconfigured), Arc::new(Unconfigured))
    };
    start_with(ex, de, label, None, None, tempdir(label)).await
}

impl T {
    fn url(&self, p: &str) -> String {
        format!("http://{}{}", self.addr, p)
    }
    async fn form(&self, p: &str, fields: &[(&str, &str)]) -> u16 {
        let mut body = format!("csrf={}", self.app.csrf);
        // forms are session-bound: default to the active session unless given
        if !fields.iter().any(|(k, _)| *k == "session_id") {
            if let Some(h) = self.app.active() {
                body.push_str(&format!("&session_id={}", h.id));
            }
        }
        if !fields.iter().any(|(k, _)| *k == "state_rev") {
            if let Some(h) = self.app.active() {
                body.push_str(&format!("&state_rev={}", h.state.lock().unwrap().state_rev));
            }
        }
        for (k, v) in fields {
            body.push_str(&format!(
                "&{k}={}",
                v.replace(' ', "+").replace('$', "%24").replace(',', "%2C")
            ));
        }
        self.http
            .post(self.url(p))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
    async fn api(&self, method: &str, p: &str, body: Option<Value>) -> (u16, Value) {
        let mut rb = if method == "GET" {
            self.http.get(self.url(p))
        } else {
            self.http.post(self.url(p))
        };
        rb = rb.bearer_auth("tok");
        if let Some(b) = body {
            rb = rb
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&b).unwrap());
        }
        let r = rb.send().await.unwrap();
        let st = r.status().as_u16();
        (
            st,
            serde_json::from_slice(&r.bytes().await.unwrap()).unwrap_or(Value::Null),
        )
    }
    async fn negotiation_session(&self) -> String {
        assert_eq!(
            self.form("/ui/session", &[("mode", "negotiation")]).await,
            303
        );
        assert_eq!(
            self.form(
                "/ui/goals",
                &[
                    ("goals_version", "1"),
                    ("size", "queen"),
                    ("firmness", "firm"),
                    ("budget", "1200"),
                    ("nights", "100")
                ]
            )
            .await,
            303
        );
        self.app.active().unwrap().id.clone()
    }
    async fn turn(&self, sid: &str, cid: &str, seq: u64, text: &str) -> (u16, Value) {
        self.api(
            "POST",
            &format!("/v1/sessions/{sid}/turns"),
            Some(json!({"protocol":"vexo-copilot/1","session_id":sid,"mode":"negotiation","source_id":"cy-bridge",
                "client_turn_id":cid,"source_seq":seq,"transcript":{"text":text,"transcript_source":"simulated"}})),
        )
        .await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn u_network_and_form_guards() {
    let t = start("mock").await;
    // Host check on GET routes too
    let r = t
        .http
        .get(t.url("/"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 421);
    let r = t
        .http
        .get(t.url("/ui/status"))
        .header("host", "evil.example:80")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 421);
    // CSRF and cross-origin
    let r = t
        .http
        .post(t.url("/ui/session"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("mode=negotiation")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let body = format!("csrf={}&mode=negotiation", t.app.csrf);
    let r = t
        .http
        .post(t.url("/ui/session"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("origin", "http://evil.example")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    let r = t
        .http
        .post(t.url("/ui/session"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "cross-site")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 403);
    // bridge auth and content type
    let r = t
        .http
        .get(t.url("/v1/sessions/active"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
    let r = t
        .http
        .post(t.url("/v1/sessions/x/turns"))
        .bearer_auth("tok")
        .header("content-type", "text/plain")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 415);
    // status panel refreshes and has no forms; main page does not refresh
    t.negotiation_session().await;
    let status = t.http.get(t.url("/ui/status")).send().await.unwrap();
    let csp = status
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(csp.contains("default-src 'none'") && csp.contains("style-src 'self'"));
    assert!(!csp.contains("unsafe-inline"));
    let status = status.text().await.unwrap();
    assert!(status.contains("http-equiv=\"refresh\"") && !status.contains("<form"));
    let index = t
        .http
        .get(t.url("/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!index.contains("http-equiv=\"refresh\"") && index.contains("<form"));
    assert!(!index.contains("<script"));
}

#[tokio::test(flavor = "multi_thread")]
async fn u_typed_and_bridge_confirmation_without_stale_loops() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    // bridge turn
    let (st, v) = t
        .turn(
            &sid,
            "b1",
            1,
            "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.",
        )
        .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["turn_outcome"], "APPLIED");
    assert_eq!(v["snapshot"]["outcome"], "ASK");
    // a second bridge turn that changes no facts does not make the confirm form stale
    let page = t
        .http
        .get(t.url("/confirm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let (st2, _) = t.turn(&sid, "b2", 2, "Thanks.").await;
    assert_eq!(st2, 200);
    let grab = |k: &str| {
        let key = format!("name=\"{k}\" value=\"");
        let i = page.find(&key).unwrap() + key.len();
        page[i..i + page[i..].find('"').unwrap()].to_string()
    };
    let code = t
        .form(
            "/ui/confirm",
            &[
                ("state_rev", &grab("state_rev")),
                ("offer_id", &grab("offer_id")),
                ("items", &grab("items").replace(',', "%2C")),
            ],
        )
        .await;
    assert_eq!(code, 303, "confirmation accepted without a 409 loop");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (_, snap) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
    assert_eq!(snap["outcome"], "PROCEED", "{snap}");
    assert_eq!(snap["provenance"]["providers"], "mock");
    assert_eq!(snap["provenance"]["decision_source"], "mock_decisions");
    assert_eq!(snap["provenance"]["input_source"], "simulated");
    // typed path through the UI
    let code = t
        .form("/ui/turn", &[("text", "$1,399."), ("nonce", "n1")])
        .await;
    assert_eq!(code, 303);
    let (_, snap) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
    assert_eq!(snap["outcome"], "PAUSE");
    assert_eq!(snap["provenance"]["input_source"], "typed");
    // lookup by client_turn_id and duplicate delivery
    let (st, v) = t
        .api("GET", &format!("/v1/sessions/{sid}/turns/b1"), None)
        .await;
    assert_eq!(st, 200);
    assert!(v["snapshot"].is_null());
    let (st, v) = t
        .turn(
            &sid,
            "b1",
            1,
            "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.",
        )
        .await;
    assert_eq!(st, 200);
    assert_eq!(v["duplicate"], true);
    assert!(
        v["snapshot"].is_null(),
        "historical duplicate carries no actionable snapshot"
    );
    let (st, v) = t.turn(&sid, "b1", 1, "different").await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (409, Some("turn_id_reused"))
    );
    let (st, v) = t
        .api("GET", &format!("/v1/sessions/{sid}/turns/nope"), None)
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (404, Some("not_accepted"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfigured_provider_is_unavailable_with_barrier() {
    let t = start("unconfigured").await;
    let sid = t.negotiation_session().await;
    let (st, v) = t.turn(&sid, "b1", 1, "$1,150 delivered").await;
    assert_eq!(st, 200);
    assert_eq!(v["turn_outcome"], "FAILED_BEFORE_APPLY");
    assert_eq!(v["error"]["code"], "missing_credentials");
    let (_, snap) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
    assert_eq!(snap["outcome"], "UNAVAILABLE");
    assert_eq!(snap["cue"]["kind"], "error");
    assert_eq!(snap["barriers"], json!([1]));
    // cancel ack is never actionable
    let (_, ack) = t
        .api(
            "POST",
            &format!("/v1/sessions/{sid}/turns/b1/cancel"),
            Some(json!({})),
        )
        .await;
    assert_eq!(ack["kind"], "cancel_ack");
    assert_eq!(ack["actionable"], false);
    assert!(ack["cue"].is_null());
    // retry ack
    let (_, ack) = t
        .api(
            "POST",
            &format!("/v1/sessions/{sid}/turns/b1/retry"),
            Some(json!({"attempt": 9})),
        )
        .await;
    assert_eq!(ack["result"], "attempt_mismatch");
}

#[tokio::test(flavor = "multi_thread")]
async fn x_body_and_poll_limits() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    let big = "x".repeat(17 * 1024);
    let r = t
        .http
        .post(t.url(&format!("/v1/sessions/{sid}/turns")))
        .bearer_auth("tok")
        .header("content-type", "application/json")
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 413);
    // 8 waiting polls admitted, the 9th rejected
    let rev = t.app.active().unwrap().state.lock().unwrap().rev;
    let mut hs = Vec::new();
    for _ in 0..8 {
        let http = t.http.clone();
        let u = t.url(&format!(
            "/v1/sessions/{sid}/cue?after_rev={rev}&wait_ms=3000"
        ));
        hs.push(tokio::spawn(async move {
            http.get(u)
                .bearer_auth("tok")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (st, v) = t
        .api(
            "GET",
            &format!("/v1/sessions/{sid}/cue?after_rev={rev}&wait_ms=3000"),
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (429, Some("too_many_pollers"))
    );
    for h in hs {
        assert_eq!(h.await.unwrap(), 200);
    }
}

/// Hold `n` requests in flight by sending headers with an unsent body.
async fn hold(addr: &str, n: usize) -> Vec<tokio::net::TcpStream> {
    let mut v = Vec::new();
    for _ in 0..n {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!("POST /v1/sessions/x/cue-reports HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer tok\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        v.push(s);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn x_combined_admission_limit_includes_polls() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    let rev = t.app.active().unwrap().state.lock().unwrap().rev;
    // 8 waiting polls + 55 held requests = 63 in flight
    let mut polls = Vec::new();
    for _ in 0..8 {
        let http = t.http.clone();
        let u = t.url(&format!(
            "/v1/sessions/{sid}/cue?after_rev={rev}&wait_ms=5000"
        ));
        polls.push(tokio::spawn(async move {
            http.get(u)
                .bearer_auth("tok")
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    let _held = hold(&t.addr, 55).await;
    assert_eq!(t.app.requests.available_permits(), 1, "63 of 64 in flight");
    let (st, _) = t.api("GET", "/v1/sessions/active", None).await;
    assert_eq!(st, 200, "64th admitted");
    let _more = hold(&t.addr, 1).await;
    assert_eq!(t.app.requests.available_permits(), 0);
    let (st, v) = t.api("GET", "/v1/sessions/active", None).await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (503, Some("server_busy"))
    );
    drop(_held);
    drop(_more);
    for p in polls {
        p.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn x9_job_slots_never_exceed_two() {
    use copilot_core::contract::ApiError;
    use copilot_providers::mock::{Scripted, ScriptedExtractor};
    let mut items = Vec::new();
    for _ in 0..20 {
        items.push(Scripted::Delay(
            50,
            Box::new(Scripted::Err(ApiError::new(
                "extraction_timeout",
                "x",
                true,
            ))),
        ));
    }
    let ex = Arc::new(ScriptedExtractor::new(items));
    let t = start_with(ex, Arc::new(MockDecider), "mock", None, None, tempdir("x9")).await;
    let sid = t.negotiation_session().await;
    let mut hs = Vec::new();
    for i in 0..20u64 {
        let http = t.http.clone();
        let u = t.url(&format!("/v1/sessions/{sid}/turns"));
        let body = json!({"protocol":"vexo-copilot/1","session_id":sid,"mode":"negotiation","source_id":"cy-bridge",
            "client_turn_id":format!("r{i}"),"source_seq":i+1,"transcript":{"text":"$1,150 delivered","transcript_source":"simulated"}});
        hs.push(tokio::spawn(async move {
            http.post(u)
                .bearer_auth("tok")
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&body).unwrap())
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    let codes: Vec<u16> = futures_join(hs).await;
    assert!(
        codes.iter().all(|c| *c == 200 || *c == 409 || *c == 429),
        "{codes:?}"
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(t.app.jobs_peak.load(Ordering::SeqCst) <= 2);
    assert_eq!(t.app.jobs_running.load(Ordering::SeqCst), 0);
    assert_eq!(t.app.slots.available_permits(), 2, "all slots returned");
}

async fn futures_join(hs: Vec<tokio::task::JoinHandle<u16>>) -> Vec<u16> {
    let mut v = Vec::new();
    for h in hs {
        v.push(h.await.unwrap());
    }
    v
}

// ---------------------------------------------------------------------------
// Memory publication, persistence failures, recovery
// ---------------------------------------------------------------------------

async fn lunch_session(t: &T) {
    assert_eq!(t.form("/ui/session", &[("mode", "lunch")]).await, 303);
    let sr = t
        .app
        .active()
        .unwrap()
        .state
        .lock()
        .unwrap()
        .state_rev
        .to_string();
    assert_eq!(
        t.form("/ui/lunch/fixture", &[("state_rev", &sr)]).await,
        303
    );
}

async fn add(t: &T, kind: PrefKind, strength: Strength) -> SaveResult {
    let v = t.app.memory.view().publication;
    t.app
        .memory
        .apply(
            v,
            Change::Add {
                kind,
                strength,
                note: String::new(),
            },
        )
        .await
}

async fn wait_outcome(t: &T, want: &str) -> Value {
    let sid = t.app.active().unwrap().id.clone();
    for _ in 0..50 {
        let (_, s) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
        if s["outcome"] == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let (_, s) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
    panic!("wanted {want}, got {s}");
}

#[tokio::test(flavor = "multi_thread")]
async fn mp_pending_allergy_blocks_selection_until_published() {
    let gate: copilot_server::memory::Gate = Arc::new(tokio::sync::Mutex::new(None));
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        Some(gate.clone()),
        tempdir("mp"),
    )
    .await;
    assert!(matches!(
        add(
            &t,
            PrefKind::MaxWalk {
                distance: copilot_core::distance::Distance::parse(
                    "1.0",
                    copilot_core::distance::DistUnit::Mi
                )
                .unwrap()
            },
            Strength::Hard
        )
        .await,
        SaveResult::Saved(_)
    ));
    lunch_session(&t).await;
    // without the peanut allergy, A (0.4 mi) and B are eligible -> mock choice
    let s = wait_outcome(&t, "ASK").await;
    assert!(
        s["selected_option"].is_null(),
        "no clear preference between A and B"
    );
    // add the allergy with the actor held between durable write and publication
    let (tx, rx) = tokio::sync::oneshot::channel();
    *gate.lock().await = Some(rx);
    let app = t.app.clone();
    let h = tokio::spawn(async move {
        let v = app.memory.view().publication;
        app.memory
            .apply(
                v,
                Change::Add {
                    kind: PrefKind::Allergen {
                        allergen: Allergen::Peanut,
                    },
                    strength: Strength::Hard,
                    note: String::new(),
                },
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(t.app.memory.view().pending_change);
    let sid = t.app.active().unwrap().id.clone();
    let (_, snap) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
    assert_ne!(
        snap["outcome"], "SELECT",
        "no selection while a preference change is pending: {snap}"
    );
    tx.send(()).unwrap();
    assert!(matches!(h.await.unwrap(), SaveResult::Saved(_)));
    let s = wait_outcome(&t, "SELECT").await;
    assert_eq!(
        s["selected_option"], "B",
        "A excluded after the allergy is published"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mp_write_failures() {
    // failure before rename: not saved, nothing changes
    let dir = tempdir("mpf");
    let path = dir.0.join("memory.json");
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        Some(FailPoint::BeforeRename),
        None,
        dir,
    )
    .await;
    let r = add(
        &t,
        PrefKind::Allergen {
            allergen: Allergen::Peanut,
        },
        Strength::Hard,
    )
    .await;
    assert!(matches!(r, SaveResult::NotSaved(_)), "{r:?}");
    assert_eq!(t.app.memory.view().version, 0);
    assert!(!path.exists());
    assert!(t.app.memory.view().recovery_required.is_none());
    // rename succeeded but directory flush failed: uncertain + recovery, never "Saved"
    let t2 = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        Some(FailPoint::DirSync),
        None,
        tempdir("mpd"),
    )
    .await;
    let r = add(
        &t2,
        PrefKind::Allergen {
            allergen: Allergen::Peanut,
        },
        Strength::Hard,
    )
    .await;
    assert!(matches!(r, SaveResult::Uncertain(_)), "{r:?}");
    assert!(t2.app.memory.view().recovery_required.is_some());
    lunch_session(&t2).await;
    let s = wait_outcome(&t2, "UNAVAILABLE").await;
    assert_eq!(s["error"]["code"], "memory_recovery_required");
}

#[tokio::test(flavor = "multi_thread")]
async fn p_queued_and_stale_edits() {
    let t = start("mock").await;
    let v0 = t.app.memory.view().publication;
    let a = t.app.memory.apply(
        v0,
        Change::Add {
            kind: PrefKind::Allergen {
                allergen: Allergen::Peanut,
            },
            strength: Strength::Hard,
            note: String::new(),
        },
    );
    let b = t.app.memory.apply(
        v0,
        Change::Add {
            kind: PrefKind::Allergen {
                allergen: Allergen::Sesame,
            },
            strength: Strength::Hard,
            note: String::new(),
        },
    );
    let (ra, rb) = tokio::join!(a, b);
    let saved = [&ra, &rb]
        .iter()
        .filter(|r| matches!(r, SaveResult::Saved(_)))
        .count();
    let stale = [&ra, &rb]
        .iter()
        .filter(|r| matches!(r, SaveResult::Stale))
        .count();
    assert_eq!((saved, stale), (1, 1), "{ra:?} {rb:?}");
    // sequential edits both apply; disk equals memory
    let v1 = t.app.memory.view().publication;
    assert!(matches!(
        t.app
            .memory
            .apply(
                v1,
                Change::Add {
                    kind: PrefKind::Allergen {
                        allergen: Allergen::Soy
                    },
                    strength: Strength::Hard,
                    note: String::new()
                }
            )
            .await,
        SaveResult::Saved(_)
    ));
    let disk: Value = serde_json::from_slice(&std::fs::read(&t.app.memory.path).unwrap()).unwrap();
    assert_eq!(
        disk["memory_version"].as_u64().unwrap(),
        t.app.memory.view().version
    );
    assert_eq!(
        disk["prefs"].as_array().unwrap().len(),
        t.app.memory.view().prefs.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn q_corrupt_memory_requires_explicit_recovery_across_restarts() {
    let dir = tempdir("q");
    let path = dir.0.join("memory.json");
    std::fs::write(&path, b"{ this is not json").unwrap();
    let dir_path = dir.0.clone();
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        dir,
    )
    .await;
    assert!(t.app.memory.view().recovery_required.is_some());
    lunch_session(&t).await;
    let s = wait_outcome(&t, "UNAVAILABLE").await;
    assert_eq!(s["error"]["code"], "memory_recovery_required");
    // file untouched; a "restart" (new app on the same dir) is still in recovery
    assert_eq!(std::fs::read(&path).unwrap(), b"{ this is not json");
    let (app2, _) = copilot_server::spawn(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        dir_path.clone(),
        None,
        None,
        None,
    )
    .await;
    assert!(app2.memory.view().recovery_required.is_some());
    // writes refused; wrong fingerprint refused; typed confirmation required in the UI
    assert!(matches!(
        add(
            &t,
            PrefKind::Allergen {
                allergen: Allergen::Peanut
            },
            Strength::Hard
        )
        .await,
        SaveResult::NotSaved(_)
    ));
    assert!(matches!(
        t.app.memory.new_profile("wrong".into()).await,
        SaveResult::Stale
    ));
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    assert_eq!(
        t.form(
            "/ui/memory/new_profile",
            &[("fingerprint", &fp), ("confirm_text", "yes")]
        )
        .await,
        422
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{ this is not json");
    assert_eq!(
        t.form(
            "/ui/memory/new_profile",
            &[("fingerprint", &fp), ("confirm_text", "START NEW PROFILE")]
        )
        .await,
        303
    );
    assert!(t.app.memory.view().recovery_required.is_none());
    let moved = std::fs::read_dir(&dir_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.corrupt-")
        });
    assert!(moved, "bad file kept");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_change_rejects_old_session_turns() {
    let t = start("mock").await;
    let old = t.negotiation_session().await;
    let _new = t.negotiation_session().await;
    let (st, v) = t.turn(&old, "z1", 1, "$1,150 delivered").await;
    assert_eq!(
        (st, v["error"]["code"].as_str()),
        (409, Some("session_closed"))
    );
    let (_, snap) = t.api("GET", &format!("/v1/sessions/{old}/cue"), None).await;
    assert_eq!(snap["phase"], "closed");
    assert_eq!(snap["cue"]["kind"], "clear");
}

#[allow(dead_code)]
async fn read_all(s: &mut tokio::net::TcpStream) -> String {
    let mut b = Vec::new();
    let _ = s.read_to_end(&mut b).await;
    String::from_utf8_lossy(&b).into()
}

// ---------------------------------------------------------------------------
// Ordering races at the publication boundary (gated MOCK decider) and deadlines
// ---------------------------------------------------------------------------

use copilot_core::contextual::ChoiceAnswer;
use copilot_providers::mock::{Scripted, ScriptedDecider, ScriptedExtractor};

fn choice(a: f64) -> ChoiceAnswer {
    let mut p = std::collections::BTreeMap::new();
    p.insert("A".to_string(), a);
    p.insert("B".to_string(), 1.0 - a);
    ChoiceAnswer {
        choice: "A".into(),
        probabilities: p,
        confidence: a,
    }
}

fn record(t: &T) -> Arc<std::sync::Mutex<Vec<Value>>> {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut rx = t.app.active().unwrap().cue_tx.subscribe();
    let s2 = seen.clone();
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let v = serde_json::to_value(&**rx.borrow()).unwrap();
            s2.lock().unwrap().push(v);
        }
    });
    seen
}

async fn gated_lunch(
    tag: &str,
) -> (
    T,
    tokio::sync::oneshot::Sender<()>,
    Arc<ScriptedDecider>,
    Arc<std::sync::Mutex<Vec<Value>>>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dec = Arc::new(ScriptedDecider::new(
        vec![],
        vec![Scripted::Gate(rx, Box::new(Scripted::Ok(choice(0.8))))],
    ));
    let t = start_with(
        Arc::new(MockExtractor),
        dec.clone(),
        "mock",
        None,
        None,
        tempdir(tag),
    )
    .await;
    let mi = copilot_core::distance::Distance::parse("1.0", copilot_core::distance::DistUnit::Mi)
        .unwrap();
    assert!(matches!(
        add(&t, PrefKind::MaxWalk { distance: mi }, Strength::Hard).await,
        SaveResult::Saved(_)
    ));
    assert_eq!(t.form("/ui/session", &[("mode", "lunch")]).await, 303);
    let seen = record(&t);
    let sr = t
        .app
        .active()
        .unwrap()
        .state
        .lock()
        .unwrap()
        .state_rev
        .to_string();
    assert_eq!(
        t.form("/ui/lunch/fixture", &[("state_rev", &sr)]).await,
        303
    );
    // wait until the choice call is in flight (A and B eligible)
    for _ in 0..50 {
        if dec.calls.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(dec.calls.load(Ordering::SeqCst), 1);
    (t, tx, dec, seen)
}

fn never_selected_a(seen: &Arc<std::sync::Mutex<Vec<Value>>>) {
    for s in seen.lock().unwrap().iter() {
        assert_ne!(s["selected_option"], "A", "stale selection published: {s}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mp2_memory_published_before_commit_discards_stale_choice() {
    let (t, tx, _dec, seen) = gated_lunch("mp2").await;
    // publication of a new hard allergy while the choice is in flight
    assert!(matches!(
        add(
            &t,
            PrefKind::Allergen {
                allergen: Allergen::Peanut
            },
            Strength::Hard
        )
        .await,
        SaveResult::Saved(_)
    ));
    tx.send(()).unwrap();
    let s = wait_outcome(&t, "SELECT").await;
    assert_eq!(s["selected_option"], "B");
    never_selected_a(&seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn l7_option_edit_during_evaluation_discards_stale_choice() {
    let (t, tx, _dec, seen) = gated_lunch("l7").await;
    let sr = t
        .app
        .active()
        .unwrap()
        .state
        .lock()
        .unwrap()
        .state_rev
        .to_string();
    assert_eq!(
        t.form(
            "/ui/lunch/fact",
            &[
                ("state_rev", &sr),
                ("opt", "A"),
                ("kind", "distance"),
                ("value", "1.5")
            ]
        )
        .await,
        303
    );
    tx.send(()).unwrap();
    let s = wait_outcome(&t, "SELECT").await;
    assert_eq!(s["selected_option"], "B");
    assert!(s["reason"].as_str().unwrap().contains("1.5 mi > 1.0 mi"));
    never_selected_a(&seen);
}

#[tokio::test(flavor = "multi_thread")]
async fn job_deadline_fails_turn_and_late_result_never_applies() {
    let late =
        copilot_providers::mock::mock_negotiation("Best I can do is $1,150 delivered.", None);
    let ex = Arc::new(ScriptedExtractor::new(vec![Scripted::Delay(
        14_600,
        Box::new(Scripted::Ok(copilot_providers::ExtractOutput::Negotiation(
            late,
        ))),
    )]));
    let t = start_with(
        ex,
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        tempdir("deadline"),
    )
    .await;
    let sid = t.negotiation_session().await;
    let (st, v) = t
        .turn(&sid, "d1", 1, "Best I can do is $1,150 delivered.")
        .await;
    assert_eq!(st, 200);
    // the 14 s job deadline fires before the 15 s response wait
    assert_eq!(v["turn_outcome"], "FAILED_BEFORE_APPLY");
    assert_eq!(v["error"]["code"], "job_deadline");
    let s = wait_outcome_long(&t, "UNAVAILABLE").await;
    assert_eq!(s["error"]["code"], "job_deadline");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let h = t.app.active().unwrap();
    let st = h.state.lock().unwrap();
    assert!(st.active.is_none(), "late extraction result never applied");
    assert_ne!(
        st.snapshot("p").outcome,
        copilot_core::contract::Outcome::Proceed
    );
    assert_eq!(t.app.slots.available_permits(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_but_valid_extraction_within_limits_succeeds() {
    let text = "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.";
    let ext = copilot_providers::mock::mock_negotiation(text, None);
    let ex = Arc::new(ScriptedExtractor::new(vec![Scripted::Delay(
        7_500,
        Box::new(Scripted::Ok(copilot_providers::ExtractOutput::Negotiation(
            ext,
        ))),
    )]));
    let t = start_with(
        ex,
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        tempdir("slowok"),
    )
    .await;
    let sid = t.negotiation_session().await;
    let (st, v) = t.turn(&sid, "s1", 1, text).await;
    assert_eq!(st, 200);
    assert_eq!(v["turn_outcome"], "APPLIED", "{v}");
    assert_eq!(v["snapshot"]["outcome"], "ASK");
    assert!(v["snapshot"]["timings_ms"]["extract"].as_u64().unwrap() >= 7_000);
}

async fn wait_outcome_long(t: &T, want: &str) -> Value {
    let sid = t.app.active().unwrap().id.clone();
    for _ in 0..100 {
        let (_, s) = t.api("GET", &format!("/v1/sessions/{sid}/cue"), None).await;
        if s["outcome"] == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never reached {want}");
}

// ---------------------------------------------------------------------------
// Review fixes: session binding, canonical errors, cue reports, CSP, latency
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn stale_session_forms_are_rejected_before_mutation() {
    let t = start("mock").await;
    let old = t.negotiation_session().await;
    let (st, _) = t
        .turn(
            &old,
            "a1",
            1,
            "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.",
        )
        .await;
    assert_eq!(st, 200);
    // capture the old session's confirmation form
    let page = t
        .http
        .get(t.url("/confirm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let grab = |k: &str| {
        let key = format!("name=\"{k}\" value=\"");
        let i = page.find(&key).unwrap() + key.len();
        page[i..i + page[i..].find('"').unwrap()].to_string()
    };
    let (sr, offer, items) = (grab("state_rev"), grab("offer_id"), grab("items"));
    // a new session with the same shape of IDs and revisions
    let new = t.negotiation_session().await;
    let (st, _) = t
        .turn(
            &new,
            "b1",
            1,
            "Best I can do is $1,000 delivered, queen, firm, with a 100-night return.",
        )
        .await;
    assert_eq!(st, 200);
    let items_enc = items.replace(',', "%2C");
    let fields = [
        ("session_id", old.as_str()),
        ("state_rev", sr.as_str()),
        ("offer_id", offer.as_str()),
        ("items", items_enc.as_str()),
    ];
    assert_eq!(t.form("/ui/confirm", &fields).await, 409);
    let h = t.app.active().unwrap();
    assert!(h
        .state
        .lock()
        .unwrap()
        .active
        .as_ref()
        .unwrap()
        .facts
        .iter()
        .all(|f| !f.confirmed));
    // goals, lunch edits and close from the old session are rejected too
    let goals = [
        ("session_id", old.as_str()),
        ("goals_version", "2"),
        ("size", "king"),
        ("firmness", "soft"),
        ("budget", "9000"),
        ("nights", "1"),
    ];
    assert_eq!(t.form("/ui/goals", &goals).await, 409);
    assert_eq!(
        t.form("/ui/close", &[("session_id", old.as_str())]).await,
        409
    );
    assert_eq!(t.app.active().unwrap().id, new);
    assert_eq!(h.state.lock().unwrap().goals.budget_cents, 120_000);
}

#[tokio::test(flavor = "multi_thread")]
async fn framework_rejections_use_the_json_envelope() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    let (st, v) = t
        .api(
            "GET",
            &format!("/v1/sessions/{sid}/cue?after_rev=abc"),
            None,
        )
        .await;
    assert_eq!(st, 400);
    assert_eq!(v["kind"], "error");
    assert_eq!(v["error"]["code"], "bad_request");
    let big = "x".repeat(17 * 1024);
    let r = t
        .http
        .post(t.url(&format!("/v1/sessions/{sid}/turns")))
        .bearer_auth("tok")
        .header("content-type", "application/json")
        .body(big)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 413);
    let v: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(v["error"]["code"], "payload_too_large");
    let r = t
        .http
        .post(t.url(&format!("/v1/sessions/{sid}/turns/x/retry")))
        .bearer_auth("tok")
        .header("content-type", "application/json")
        .body("{\"attempt\":\"one\"}")
        .send()
        .await
        .unwrap();
    assert!(r.status().is_client_error());
    let v: Value = serde_json::from_slice(&r.bytes().await.unwrap()).unwrap();
    assert_eq!(v["kind"], "error");
}

#[tokio::test(flavor = "multi_thread")]
async fn cue_reports_are_validated_before_ack() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    let rev = t.app.active().unwrap().state.lock().unwrap().rev;
    let bad = [
        json!({"protocol":"vexo-copilot/1","rev":rev,"applied":"blink","bridge_durations_ms":{"submit_to_final_cue_applied_ms":5}}),
        json!({"protocol":"vexo-copilot/1","rev":rev + 1000,"applied":"led_set","bridge_durations_ms":{"submit_to_final_cue_applied_ms":5}}),
        json!({"protocol":"vexo-copilot/1","rev":rev,"applied":"led_set","bridge_durations_ms":{"made_up":5}}),
        json!({"protocol":"vexo-copilot/1","rev":rev,"applied":"led_set","bridge_durations_ms":{"submit_to_final_cue_applied_ms":5},"extra":1}),
    ];
    for b in bad {
        let (st, v) = t
            .api("POST", &format!("/v1/sessions/{sid}/cue-reports"), Some(b))
            .await;
        assert!(st == 400 || st == 422, "{st} {v}");
        assert_eq!(v["kind"], "error");
    }
    let ok = json!({"protocol":"vexo-copilot/1","rev":rev,"applied":"led_set","bridge_durations_ms":{"submit_to_final_cue_applied_ms":5400}});
    let (st, v) = t
        .api("POST", &format!("/v1/sessions/{sid}/cue-reports"), Some(ok))
        .await;
    assert_eq!((st, v["kind"].as_str()), (200, Some("report_ack")));
    assert_eq!(v["actionable"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn styles_are_served_without_inline_css() {
    let t = start("mock").await;
    t.negotiation_session().await;
    let r = t.http.get(t.url("/static/app.css")).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 200);
    assert!(r.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/css"));
    assert!(r.text().await.unwrap().contains(".cue-green"));
    for p in ["/", "/ui/status", "/confirm"] {
        let html = t
            .http
            .get(t.url(p))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            !html.contains("<style") && !html.contains(" style="),
            "{p} has inline styles"
        );
        assert!(html.contains("/static/app.css"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn three_second_confirmation_pause_is_measured() {
    let t = start("mock").await;
    let sid = t.negotiation_session().await;
    let (st, v) = t
        .turn(
            &sid,
            "l1",
            1,
            "Best I can do is $1,150 delivered, queen, firm, with a 100-night return.",
        )
        .await;
    assert_eq!(
        (st, v["snapshot"]["phase"].as_str()),
        (200, Some("awaiting_confirmation"))
    );
    tokio::time::sleep(Duration::from_millis(3_000)).await;
    let page = t
        .http
        .get(t.url("/confirm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let grab = |k: &str| {
        let key = format!("name=\"{k}\" value=\"");
        let i = page.find(&key).unwrap() + key.len();
        page[i..i + page[i..].find('"').unwrap()].to_string()
    };
    let items = grab("items").replace(',', "%2C");
    assert_eq!(
        t.form(
            "/ui/confirm",
            &[("offer_id", &grab("offer_id")), ("items", &items)]
        )
        .await,
        303
    );
    let s = wait_outcome(&t, "PROCEED").await;
    let tm = &s["timings_ms"];
    assert!(tm["e2e"].as_u64().unwrap() >= 3_000, "{tm}");
    assert!(tm["human_wait"].as_u64().unwrap() >= 3_000, "{tm}");
    assert!(tm["processing"].as_u64().unwrap() < 1_000, "{tm}");
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_load_validation_and_fingerprint_recheck() {
    // a semantically invalid file (hard avoid preference) requires recovery
    let dir = tempdir("memval");
    let path = dir.0.join("memory.json");
    let doc = json!({"schema_version":1,"memory_version":1,"next_id":2,"prefs":[
        {"id":1,"version":1,"kind":{"type":"diet","diet":"vegetarian","prefer":false},"strength":"hard","note":"","provenance":"user entry via UI"}]});
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        dir,
    )
    .await;
    let r = t.app.memory.view().recovery_required.unwrap();
    assert!(r.contains("validation"), "{r}");
    // the file changes after the user saw it: new profile is refused
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    std::fs::write(&path, b"{\"changed\": true}").unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp).await,
        SaveResult::Stale
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"changed\": true}");
}

// ---------------------------------------------------------------------------
// Second local review: memory publication, profile reset, oversized identity
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn restoring_an_older_file_keeps_publications_monotonic() {
    let dir = tempdir("older");
    let path = dir.0.join("memory.json");
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        dir,
    )
    .await;
    let mi = copilot_core::distance::Distance::parse("1.0", copilot_core::distance::DistUnit::Mi)
        .unwrap();
    assert!(matches!(
        add(&t, PrefKind::MaxWalk { distance: mi }, Strength::Hard).await,
        SaveResult::Saved(1)
    ));
    let older = std::fs::read(&path).unwrap();
    assert!(matches!(
        add(
            &t,
            PrefKind::Allergen {
                allergen: Allergen::Peanut
            },
            Strength::Hard
        )
        .await,
        SaveResult::Saved(2)
    ));
    lunch_session(&t).await;
    let s = wait_outcome(&t, "SELECT").await;
    assert_eq!(s["selected_option"], "B");
    let before = t.app.memory.view().publication;
    // the user restores the older file (no peanut allergy) and re-checks
    std::fs::write(&path, &older).unwrap();
    assert!(matches!(t.app.memory.recheck().await, SaveResult::Saved(1)));
    let v = t.app.memory.view();
    assert!(
        v.publication > before,
        "publication {} > {before}",
        v.publication
    );
    assert_eq!(v.version, 1);
    // the lunch session re-evaluates against the restored file and its queue drains
    let s = wait_outcome(&t, "ASK").await; // A and B eligible again, no soft preference
    assert!(s["selected_option"].is_null());
    tokio::time::sleep(Duration::from_millis(200)).await;
    let h = t.app.active().unwrap();
    assert!(h.state.lock().unwrap().queue.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_profile_reset_keeps_recovery_across_restart() {
    let dir = tempdir("reset");
    let path = dir.0.join("memory.json");
    let dir_path = dir.0.clone();
    std::fs::write(&path, b"{ corrupt").unwrap();
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        Some(FailPoint::BeforeRename),
        None,
        dir,
    )
    .await;
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp).await,
        SaveResult::NotSaved(_)
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{ corrupt",
        "memory.json never missing"
    );
    assert!(t.app.memory.view().recovery_required.is_some());
    let quarantined = std::fs::read_dir(&dir_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.corrupt-")
        })
        .count();
    assert_eq!(
        quarantined, 0,
        "failed reset leaves no stray quarantine copy"
    );
    let (app2, _) = copilot_server::spawn(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        dir_path,
        None,
        None,
        None,
    )
    .await;
    assert!(
        app2.memory.view().recovery_required.is_some(),
        "recovery survives restart"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_files_are_identified_by_content_and_quarantine_names_are_unique() {
    let dir = tempdir("oversize");
    let path = dir.0.join("memory.json");
    let dir_path = dir.0.clone();
    let n = copilot_core::limits::MEMORY_FILE_MAX + 10;
    std::fs::write(&path, vec![b'a'; n]).unwrap();
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        dir,
    )
    .await;
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    // same length, different content: detected before reset
    std::fs::write(&path, vec![b'b'; n]).unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp).await,
        SaveResult::Stale
    ));
    assert_eq!(std::fs::read(&path).unwrap()[0], b'b');
    // re-check picks up the current file; reset it, corrupt it again, reset again
    assert!(matches!(
        t.app.memory.recheck().await,
        SaveResult::NotSaved(_)
    ));
    let fp2 = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp2).await,
        SaveResult::Saved(0)
    ));
    std::fs::write(&path, b"{ corrupt again").unwrap();
    assert!(matches!(
        t.app.memory.recheck().await,
        SaveResult::NotSaved(_)
    ));
    let fp3 = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp3).await,
        SaveResult::Saved(0)
    ));
    let names: Vec<String> = std::fs::read_dir(&dir_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("memory.corrupt-"))
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert_ne!(names[0], names[1]);
    assert!(t.app.memory.view().recovery_required.is_none());
}

// ---------------------------------------------------------------------------
// Third local review: edits bound to publication; post-rename reset failures
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn stale_preference_edits_after_reset_or_restore_are_rejected() {
    let dir = tempdir("pubbind");
    let path = dir.0.join("memory.json");
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        None,
        None,
        dir,
    )
    .await;
    // 1. peanut allergy saved; keep its delete request (publication + id/version)
    assert!(matches!(
        add(
            &t,
            PrefKind::Allergen {
                allergen: Allergen::Peanut
            },
            Strength::Hard
        )
        .await,
        SaveResult::Saved(1)
    ));
    let old_pub = t.app.memory.view().publication;
    let old_pref = t.app.memory.view().prefs[0].clone();
    // 2. corrupt, re-check, start a new profile
    std::fs::write(&path, b"{ corrupt").unwrap();
    assert!(matches!(
        t.app.memory.recheck().await,
        SaveResult::NotSaved(_)
    ));
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    assert!(matches!(
        t.app.memory.new_profile(fp).await,
        SaveResult::Saved(0)
    ));
    // 3. sesame allergy at file version 1 with preference id 1 again
    assert!(matches!(
        add(
            &t,
            PrefKind::Allergen {
                allergen: Allergen::Sesame
            },
            Strength::Hard
        )
        .await,
        SaveResult::Saved(1)
    ));
    let now = t.app.memory.view();
    assert_eq!(
        (now.prefs[0].id, now.prefs[0].version, now.version),
        (old_pref.id, old_pref.version, 1)
    );
    // 4. the old delete and an old add are stale: nothing changes on disk or in memory
    let disk_before = std::fs::read(&path).unwrap();
    let r = t
        .app
        .memory
        .apply(
            old_pub,
            Change::Delete {
                id: old_pref.id,
                version: old_pref.version,
            },
        )
        .await;
    assert!(matches!(r, SaveResult::Stale), "{r:?}");
    let r = t
        .app
        .memory
        .apply(
            old_pub,
            Change::Add {
                kind: PrefKind::Allergen {
                    allergen: Allergen::Soy,
                },
                strength: Strength::Hard,
                note: String::new(),
            },
        )
        .await;
    assert!(matches!(r, SaveResult::Stale), "{r:?}");
    assert_eq!(std::fs::read(&path).unwrap(), disk_before);
    assert!(matches!(
        t.app.memory.view().prefs[0].kind,
        PrefKind::Allergen {
            allergen: Allergen::Sesame
        }
    ));
    // through HTTP the stale form returns 409
    let old = old_pub.to_string();
    let code = t
        .form(
            "/ui/prefs/delete",
            &[("memory_publication", &old), ("id", "1"), ("version", "1")],
        )
        .await;
    assert_eq!(code, 409);
    assert_eq!(t.app.memory.view().prefs.len(), 1);
    // restoring an older file also invalidates forms rendered before the restore
    let before_restore = t.app.memory.view().publication;
    std::fs::write(&path, &disk_before).unwrap();
    assert!(matches!(t.app.memory.recheck().await, SaveResult::Saved(1)));
    let r = t
        .app
        .memory
        .apply(before_restore, Change::Delete { id: 1, version: 1 })
        .await;
    assert!(matches!(r, SaveResult::Stale), "{r:?}");
    assert_eq!(t.app.memory.view().prefs.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn post_rename_reset_failure_is_uncertain_and_consistent_with_disk() {
    let dir = tempdir("resetsync");
    let path = dir.0.join("memory.json");
    let dir_path = dir.0.clone();
    std::fs::write(&path, b"{ corrupt").unwrap();
    let t = start_with(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        Some(FailPoint::DirSync),
        None,
        dir,
    )
    .await;
    let fp = t
        .app
        .memory
        .publ
        .lock()
        .unwrap()
        .fingerprint
        .clone()
        .unwrap();
    let r = t.app.memory.new_profile(fp).await;
    assert!(
        matches!(r, SaveResult::Uncertain(_)),
        "never claims 'not written' after the rename: {r:?}"
    );
    // published state matches disk: the empty profile, with recovery until Re-check
    let disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(disk["prefs"], json!([]));
    let v = t.app.memory.view();
    assert!(v.prefs.is_empty());
    assert!(v
        .recovery_required
        .as_deref()
        .is_some_and(|r| r.starts_with("durability_unknown")));
    // lunch stays blocked while durability is unknown
    lunch_session(&t).await;
    let s = wait_outcome(&t, "UNAVAILABLE").await;
    assert_eq!(s["error"]["code"], "memory_recovery_required");
    // the quarantine copy of the corrupt file is kept
    let kept = std::fs::read_dir(&dir_path)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("memory.corrupt-")
        })
        .count();
    assert_eq!(kept, 1);
    // Re-check (real directory flush) clears recovery in the running process
    assert!(matches!(t.app.memory.recheck().await, SaveResult::Saved(0)));
    assert!(t.app.memory.view().recovery_required.is_none());
    // restart semantics: the replaced file loads as the (explicitly requested) empty profile
    let (app2, _) = copilot_server::spawn(
        Arc::new(MockExtractor),
        Arc::new(MockDecider),
        "mock",
        dir_path,
        None,
        None,
        None,
    )
    .await;
    assert!(app2.memory.view().recovery_required.is_none());
    assert!(app2.memory.view().prefs.is_empty());
}
