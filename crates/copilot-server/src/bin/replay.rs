//! replay: SIMULATED bridge + simulated user for scripted demos.
//! Sends turns through the bridge API (transcript_source "simulated"), confirms
//! through the session-bound UI forms (X-Copilot-Simulated), applies cues with the
//! reference bridge model, and reports latency per generation: from the turn's
//! submission through the final applied cue, including every confirmation wait.

use copilot_core::bridge_model::{Action, BridgeModel};
use copilot_core::contract::{CueSnapshot, Mode, Phase};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

struct Ctx {
    base: String,
    token: String,
    http: reqwest::Client,
    csrf: String,
    model: BridgeModel,
    session: String,
    mode: Mode,
    instance: String,
    seq: u64,
}

fn scrape(html: &str, name: &str) -> Option<String> {
    let key = format!("name=\"{name}\" value=\"");
    let i = html.find(&key)? + key.len();
    let j = html[i..].find('"')? + i;
    Some(html[i..j].to_string())
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

impl Ctx {
    async fn get_text(&self, path: &str) -> String {
        let r = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            .expect("GET");
        r.text().await.unwrap_or_default()
    }
    /// Submit a UI form; session_id and state_rev are scraped from `page` (or the
    /// main page) unless given, so forms stay bound to the session they came from.
    async fn form(&self, path: &str, fields: &[(&str, String)], page: Option<&str>) -> u16 {
        let fresh;
        let page = match page {
            Some(p) => p,
            None => {
                fresh = self.get_text("/").await;
                &fresh
            }
        };
        let mut body = format!("csrf={}", self.csrf);
        for k in ["session_id", "state_rev"] {
            if !fields.iter().any(|(f, _)| *f == k) {
                if let Some(v) = scrape(page, k) {
                    body.push_str(&format!("&{k}={}", enc(&v)));
                }
            }
        }
        for (k, v) in fields {
            body.push_str(&format!("&{k}={}", enc(v)));
        }
        let r = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-copilot-simulated", "1")
            .body(body)
            .send()
            .await
            .expect("POST");
        r.status().as_u16()
    }
    async fn api_get(&self, path: &str) -> Value {
        let r = self
            .http
            .get(format!("{}{}", self.base, path))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("api GET");
        serde_json::from_slice(&r.bytes().await.unwrap_or_default()).unwrap_or(Value::Null)
    }
    async fn poll(&mut self, after: u64, wait: u64) -> Option<CueSnapshot> {
        let v = self
            .api_get(&format!(
                "/v1/sessions/{}/cue?after_rev={after}&wait_ms={wait}",
                self.session
            ))
            .await;
        serde_json::from_value(v).ok()
    }
}

async fn setup(ctx: &mut Ctx, sc: &Value) {
    let page = ctx.get_text("/").await;
    ctx.csrf = scrape(&page, "csrf").expect("csrf");
    let mode = sc["mode"].as_str().unwrap_or("negotiation");
    ctx.form("/ui/session", &[("mode", mode.into())], Some(""))
        .await;
    let info = ctx.api_get("/v1/sessions/active").await;
    ctx.session = info["session_id"].as_str().expect("session").to_string();
    ctx.instance = info["instance_id"].as_str().expect("instance").to_string();
    ctx.mode = if mode == "lunch" {
        Mode::Lunch
    } else {
        Mode::Negotiation
    };
    ctx.model.on_session(&ctx.instance, &ctx.session, ctx.mode);
    ctx.seq = 0;
    let page = ctx.get_text("/").await;
    if ctx.mode == Mode::Negotiation {
        let g = &sc["goals"];
        let gv = scrape(&page, "goals_version").unwrap_or_else(|| "1".into());
        ctx.form(
            "/ui/goals",
            &[
                ("goals_version", gv),
                ("size", g["size"].as_str().unwrap_or("queen").into()),
                ("firmness", g["firmness"].as_str().unwrap_or("firm").into()),
                ("budget", g["budget"].as_u64().unwrap_or(1200).to_string()),
                ("nights", g["nights"].as_u64().unwrap_or(100).to_string()),
            ],
            Some(&page),
        )
        .await;
    } else {
        for p in sc["prefs"].as_array().cloned().unwrap_or_default() {
            let page = ctx.get_text("/").await;
            let mv = scrape(&page, "memory_publication").unwrap_or_else(|| "0".into());
            ctx.form(
                "/ui/prefs/add",
                &[
                    ("memory_publication", mv),
                    ("kind", p["kind"].as_str().unwrap_or("").into()),
                    ("value", p["value"].as_str().unwrap_or("").into()),
                ],
                Some(&page),
            )
            .await;
        }
        ctx.form("/ui/lunch/fixture", &[], None).await;
    }
}

async fn send_turn(ctx: &mut Ctx, text: &str) -> Value {
    let (id, _) = ctx.model.capture_start();
    ctx.model.mic_stopped();
    ctx.seq += 1;
    let body = json!({
        "protocol": "vexo-copilot/1", "session_id": ctx.session, "mode": ctx.mode, "source_id": "cy-bridge",
        "client_turn_id": id, "source_seq": ctx.seq,
        "transcript": {"text": text, "transcript_source": "simulated", "speaker": "counterpart"}
    });
    let r = ctx
        .http
        .post(format!("{}/v1/sessions/{}/turns", ctx.base, ctx.session))
        .bearer_auth(&ctx.token)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&body).expect("json"))
        .send()
        .await
        .expect("turn");
    let v: Value =
        serde_json::from_slice(&r.bytes().await.unwrap_or_default()).unwrap_or(Value::Null);
    let seq = v.pointer("/turn/server_seq").and_then(Value::as_u64);
    ctx.model
        .on_turn_response(&ctx.instance, &ctx.session, ctx.mode, &id, seq);
    v
}

/// Poll until the model applies a settled cue (not processing/invalidated). The
/// budget covers queue wait plus a full job (and a re-evaluation) plus margin.
async fn settle(ctx: &mut Ctx) -> Option<CueSnapshot> {
    let budget =
        copilot_core::limits::QUEUE_WAIT_MAX_MS + 2 * copilot_core::limits::JOB_DEADLINE_MS + 5_000;
    let deadline = Instant::now() + Duration::from_millis(budget);
    let mut after = 0;
    let mut last = None;
    while Instant::now() < deadline {
        let Some(s) = ctx.poll(after, 2000).await else {
            break;
        };
        after = s.header.rev;
        if s.header.rev <= ctx.model.last_rev {
            // already applied; settled if it is not transitional
            if !matches!(s.phase, Phase::Processing | Phase::Invalidated) {
                return Some(s);
            }
            continue;
        }
        if let Action::Apply { .. } = ctx.model.on_snapshot(&s) {
            let settled = !matches!(s.phase, Phase::Processing | Phase::Invalidated);
            last = Some(s.clone());
            if settled {
                return last;
            }
        }
    }
    last
}

/// Simulated user confirming: page load counts as human time; the POST is processing.
async fn confirm_all(ctx: &mut Ctx) -> (u16, Duration) {
    let t = Instant::now();
    let page = ctx.get_text("/confirm").await;
    let human = t.elapsed();
    let code = match (scrape(&page, "items"), scrape(&page, "offer_id")) {
        (Some(items), Some(offer)) => {
            ctx.form(
                "/ui/confirm",
                &[("offer_id", offer), ("items", items)],
                Some(&page),
            )
            .await
        }
        (Some(items), None) => {
            ctx.form("/ui/lunch/confirm", &[("items", items)], Some(&page))
                .await
        }
        _ => 0,
    };
    (code, human)
}

fn outcome_str(s: &CueSnapshot) -> String {
    serde_json::to_value(s.outcome)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

/// Fill 5 sessions to their retention limits with max-size Unicode turns, hold 55
/// slow requests + 8 waiting polls, churn 200 sessions, and ASSERT occupancy
/// against the server's limits. SIMULATED load.
async fn stress_retention(mut ctx: Ctx) -> i32 {
    let pad: String = std::iter::repeat_n('\u{1F642}', 3300).collect();
    let sc = json!({"mode": "negotiation", "goals": {}});
    for sess in 0..5 {
        setup(&mut ctx, &sc).await;
        for t in 0..64u64 {
            let mut text = String::new();
            for k in 0..10 {
                text.push_str(&format!("${},1{:02} delivered, ", 1 + (t % 9), k));
            }
            text.push_str(&pad);
            let text: String = text.chars().take(4000).collect();
            let v = send_turn(&mut ctx, &text).await;
            if v["turn_outcome"].is_null() {
                eprintln!("session {sess} turn {t}: {v}");
            }
        }
        println!("session {} filled: 64 turns x 4000 chars", sess + 1);
    }
    let addr = ctx.base.trim_start_matches("http://").to_string();
    let mut held = Vec::new();
    for _ in 0..55 {
        if let Ok(mut s) = tokio::net::TcpStream::connect(&addr).await {
            use tokio::io::AsyncWriteExt;
            let req = format!(
                "POST /v1/sessions/x/cue-reports HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: 16000\r\n\r\n",
                ctx.token
            );
            let _ = s.write_all(req.as_bytes()).await;
            let _ = s.write_all(&[b' '; 15000]).await;
            held.push(s);
        }
    }
    let mut polls = Vec::new();
    for _ in 0..8 {
        let http = ctx.http.clone();
        let u = format!(
            "{}/v1/sessions/{}/cue?after_rev=999999&wait_ms=4000",
            ctx.base, ctx.session
        );
        let tok = ctx.token.clone();
        polls.push(tokio::spawn(async move {
            http.get(u)
                .bearer_auth(tok)
                .send()
                .await
                .map(|r| r.status().as_u16())
                .unwrap_or(0)
        }));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let occ_busy = ctx.api_get("/v1/diagnostics/occupancy").await;
    println!(
        "holding {} slow requests + 8 waiting polls: requests_in_flight={} waiting_polls={}",
        held.len(),
        occ_busy["requests_in_flight"],
        occ_busy["waiting_polls"]
    );
    for p in polls {
        let _ = p.await;
    }
    drop(held);
    let occ_full = ctx.api_get("/v1/diagnostics/occupancy").await;
    for i in 0..200 {
        let mode = if i % 2 == 0 { "negotiation" } else { "lunch" };
        ctx.form("/ui/session", &[("mode", mode.into())], Some(""))
            .await;
    }
    println!("churned 200 sessions");
    let occ_end = ctx.api_get("/v1/diagnostics/occupancy").await;
    // assert actual occupancy against the published limits
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        println!("  {} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };
    let lim = &occ_full["limits"];
    for (label, occ) in [("after fill", &occ_full), ("after churn", &occ_end)] {
        let sessions = occ["sessions"].as_array().cloned().unwrap_or_default();
        check(
            &format!("{label} sessions retained"),
            occ["sessions_retained"].as_u64() <= lim["sessions"].as_u64(),
            format!("{} <= {}", occ["sessions_retained"], lim["sessions"]),
        );
        for key in [
            "turns",
            "transcript_bytes",
            "archived_offers",
            "tombstones",
            "offer_turn_seqs",
            "snapshot_bytes",
            "offer_unresolved",
            "offer_conditions",
            "lunch_unresolved",
        ] {
            let max = sessions
                .iter()
                .filter_map(|s| s[key].as_u64())
                .max()
                .unwrap_or(0);
            let limit_key = if key == "turns" { "turns" } else { key };
            check(
                &format!("{label} max {key}"),
                Some(max) <= lim[limit_key].as_u64(),
                format!("{max} <= {}", lim[limit_key]),
            );
        }
        let max_tr = sessions
            .iter()
            .filter_map(|s| s["max_turn_result_bytes"].as_u64())
            .max()
            .unwrap_or(0);
        check(
            &format!("{label} max turn_result bytes"),
            Some(max_tr) <= lim["turn_result_bytes"].as_u64(),
            format!("{max_tr} <= {}", lim["turn_result_bytes"]),
        );
        let unresolved = sessions
            .iter()
            .map(|s| s["barriers"].as_u64().unwrap_or(0) + s["ambiguous"].as_u64().unwrap_or(0))
            .max()
            .unwrap_or(0);
        check(
            &format!("{label} unresolved turns"),
            Some(unresolved) <= lim["unresolved_turns"].as_u64(),
            format!("{unresolved} <= {}", lim["unresolved_turns"]),
        );
    }
    check(
        "filled sessions reached the turn limit",
        occ_full["sessions"].as_array().is_some_and(|v| {
            v.iter()
                .any(|s| s["turns"].as_u64() == lim["turns"].as_u64())
        }),
        "at least one session at 64 turns".into(),
    );
    check(
        "jobs_peak <= 2",
        occ_end["jobs_peak"].as_u64() <= Some(2),
        format!("{}", occ_end["jobs_peak"]),
    );
    check(
        "jobs drained",
        occ_end["jobs_running"].as_u64() == Some(0),
        format!("{}", occ_end["jobs_running"]),
    );
    println!("# stress occupancy failures: {failures}");
    failures
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let base = arg("--base").unwrap_or_else(|| "http://127.0.0.1:8787".into());
    let mut ctx = Ctx {
        base,
        token: std::env::var("COPILOT_BRIDGE_TOKEN").unwrap_or_default(),
        http: reqwest::Client::new(),
        csrf: String::new(),
        model: BridgeModel::new("replay"),
        session: String::new(),
        mode: Mode::Negotiation,
        instance: String::new(),
        seq: 0,
    };
    if args.iter().any(|a| a == "--stress-retention") {
        let page = ctx.get_text("/").await;
        ctx.csrf = scrape(&page, "csrf").expect("csrf");
        let f = stress_retention(ctx).await;
        std::process::exit(if f == 0 { 0 } else { 1 });
    }
    let Some(path) = arg("--scenario") else {
        eprintln!("usage: replay --scenario <file.json> | --stress-retention [--base http://127.0.0.1:8787]  (env COPILOT_BRIDGE_TOKEN)");
        std::process::exit(2);
    };
    let sc: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("scenario file"))
        .expect("scenario json");
    setup(&mut ctx, &sc).await;
    println!(
        "# scenario {}  input=SIMULATED (replay)  bridge=reference model  clock=replay process (monotonic)",
        sc["name"].as_str().unwrap_or(&path)
    );
    let mut failures = 0;
    // generation clock: starts at a turn's submission and spans its confirmations
    let mut gen_start = Instant::now();
    let mut gen_human = Duration::ZERO;
    for (i, step) in sc["steps"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let mut label = String::new();
        if let Some(t) = step["turn"].as_str() {
            gen_start = Instant::now();
            gen_human = Duration::ZERO;
            let v = send_turn(&mut ctx, t).await;
            label = format!(
                "turn \"{t}\" -> {}",
                v["turn_outcome"].as_str().unwrap_or("?")
            );
        }
        if let Some(sf) = step.get("set_fact") {
            let w = Instant::now();
            let code = ctx
                .form(
                    "/ui/lunch/fact",
                    &[
                        ("opt", sf["opt"].as_str().unwrap_or("A").into()),
                        ("kind", sf["kind"].as_str().unwrap_or("").into()),
                        ("value", sf["value"].as_str().unwrap_or("").into()),
                    ],
                    None,
                )
                .await;
            gen_human += w.elapsed();
            label = format!(
                "user sets {} {} = {} ({code})",
                sf["opt"], sf["kind"], sf["value"]
            );
        }
        if let Some(p) = step["pause_ms"].as_u64() {
            tokio::time::sleep(Duration::from_millis(p)).await;
            gen_human += Duration::from_millis(p);
            label = format!("{label} [user pauses {p} ms]");
        }
        let snap = if step["confirm"].as_bool() == Some(true) {
            let (code, human) = confirm_all(&mut ctx).await;
            gen_human += human;
            label = format!("{label} confirm({code})");
            settle(&mut ctx).await
        } else {
            settle(&mut ctx).await
        };
        let e2e = gen_start.elapsed();
        let processing = e2e.saturating_sub(gen_human);
        let (outcome, cue, sel, reason, prov, server) = match &snap {
            Some(s) => (
                outcome_str(s),
                s.cue
                    .as_ref()
                    .map(|c| format!("{} {}", c.name, c.pattern))
                    .unwrap_or("-".into()),
                s.selected_option.clone().unwrap_or("-".into()),
                s.reason.clone().unwrap_or_default(),
                format!(
                    "providers={} pipeline={} decision_source={}",
                    s.provenance.providers, s.provenance.pipeline, s.provenance.decision_source
                ),
                format!(
                    "server e2e {} ms, human wait {} ms, processing {} ms",
                    s.timings_ms
                        .e2e
                        .map(|v| v.to_string())
                        .unwrap_or("-".into()),
                    s.timings_ms
                        .human_wait
                        .map(|v| v.to_string())
                        .unwrap_or("-".into()),
                    s.timings_ms
                        .processing
                        .map(|v| v.to_string())
                        .unwrap_or("-".into())
                ),
            ),
            None => (
                "NO_CUE".into(),
                "-".into(),
                "-".into(),
                String::new(),
                String::new(),
                String::new(),
            ),
        };
        let expect = step["expect"].as_str().unwrap_or("");
        let ok = expect.is_empty() || outcome == expect;
        let sel_ok = step["expect_option"].as_str().is_none_or(|o| o == sel);
        if !(ok && sel_ok) {
            failures += 1;
        }
        println!(
            "{:>2} {} | outcome {outcome} cue {cue} option {sel} | expect {expect}{} | {} | replay generation e2e {} ms (human wait {} ms, processing {} ms) | {server} | {prov}\n   reason: {reason}",
            i + 1,
            label,
            step["expect_option"].as_str().map(|o| format!(" option {o}")).unwrap_or_default(),
            if ok && sel_ok { "PASS" } else { "FAIL" },
            e2e.as_millis(),
            gen_human.as_millis(),
            processing.as_millis()
        );
    }
    println!("# {failures} failure(s)");
    std::process::exit(if failures == 0 { 0 } else { 1 });
}
