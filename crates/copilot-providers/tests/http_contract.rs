//! Provider adapters against an in-process MOCK HTTP server (not Gemini).
//! Exercises the documented request paths/headers and every failure mapping.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use copilot_core::contract::{ApiError, Outcome};
use copilot_core::judge::Judgment;
use copilot_providers::{
    Decider, ExtractRequest, Extractor, GeminiConfig, GeminiDecider, GeminiExtractor, Secret,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
enum Behaviour {
    Json(u16, Value),
    Raw(u16, Vec<u8>),
    Delay(u64, Box<Behaviour>),
    Close,
}

#[derive(Clone, Default)]
struct Seen {
    path: Arc<Mutex<Vec<String>>>,
    key: Arc<Mutex<Vec<String>>>,
    body: Arc<Mutex<Vec<Value>>>,
}

#[derive(Clone)]
struct S {
    b: Arc<Mutex<Behaviour>>,
    seen: Seen,
}

async fn respond(b: Behaviour) -> Response {
    match b {
        Behaviour::Json(st, v) => {
            (StatusCode::from_u16(st).unwrap(), axum::Json(v)).into_response()
        }
        Behaviour::Raw(st, v) => (StatusCode::from_u16(st).unwrap(), v).into_response(),
        Behaviour::Delay(ms, next) => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Box::pin(respond(*next)).await
        }
        Behaviour::Close => panic!("drop connection"),
    }
}

async fn handler(State(s): State<S>, uri: axum::http::Uri, h: HeaderMap, body: Bytes) -> Response {
    s.seen.path.lock().unwrap().push(uri.path().to_string());
    s.seen.key.lock().unwrap().push(
        h.get("x-goog-api-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    );
    s.seen
        .body
        .lock()
        .unwrap()
        .push(serde_json::from_slice(&body).unwrap_or(Value::Null));
    let b = s.b.lock().unwrap().clone();
    respond(b).await
}

async fn server(b: Behaviour) -> (String, Seen) {
    let seen = Seen::default();
    let st = S {
        b: Arc::new(Mutex::new(b)),
        seen: seen.clone(),
    };
    let app = Router::new()
        .route("/v1beta/models/{m}", post(handler))
        .route("/v1beta/decisions", post(handler))
        .with_state(st);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    (format!("http://{addr}"), seen)
}

fn cfg(base: &str) -> GeminiConfig {
    GeminiConfig {
        api_key: Secret::new("test-key-not-real".into()),
        base_url: base.into(),
        extract_model: "gemini-3.8-flash".into(),
        thinking_level: "LOW".into(),
        decision_model: "gemini-decision-1".into(),
    }
}

fn candidate(text: &str, finish: &str) -> Value {
    json!({"candidates": [{"finishReason": finish, "content": {"parts": [{"text": text}]}}]})
}

/// The negotiation judge over generateContent (same transport as lunch extraction).
async fn extract(b: Behaviour) -> (Result<Judgment, ApiError>, Seen) {
    let (base, seen) = server(b).await;
    let ex = GeminiExtractor {
        cfg: cfg(&base),
        client: copilot_providers::http_client(),
    };
    let r = ex
        .judge("firm, $100".into(), "speaker1: $1,150 delivered.".into())
        .await;
    (r, seen)
}

#[tokio::test]
async fn extraction_success_uses_documented_request() {
    let out = json!({"outcome":"PAUSE","reason":"Over $100."}).to_string();
    let (r, seen) = extract(Behaviour::Json(200, candidate(&out, "STOP"))).await;
    assert_eq!(r.unwrap().outcome, Outcome::Pause);
    assert_eq!(
        seen.path.lock().unwrap()[0],
        "/v1beta/models/gemini-3.8-flash:generateContent"
    );
    assert_eq!(seen.key.lock().unwrap()[0], "test-key-not-real");
    let body = &seen.body.lock().unwrap()[0];
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "LOW"
    );
    assert_eq!(
        body["generationConfig"]["responseFormat"]["text"]["mimeType"],
        "APPLICATION_JSON"
    );
}

#[tokio::test]
async fn extraction_failure_map() {
    let cases: Vec<(Behaviour, &str)> = vec![
        (
            Behaviour::Json(200, json!({"promptFeedback": {"blockReason": "SAFETY"}})),
            "extraction_blocked",
        ),
        (
            Behaviour::Json(200, json!({"candidates": []})),
            "extraction_no_candidate",
        ),
        (
            Behaviour::Json(200, candidate("{", "MAX_TOKENS")),
            "extraction_truncated",
        ),
        (
            Behaviour::Json(200, candidate("x", "RECITATION")),
            "extraction_blocked",
        ),
        (
            Behaviour::Json(200, candidate("x", "OTHER")),
            "extraction_incomplete",
        ),
        (
            Behaviour::Json(200, candidate("not json", "STOP")),
            "judge_invalid_output",
        ),
        (
            Behaviour::Json(200, candidate(r#"{"outcome":"MAYBE","reason":"x"}"#, "STOP")),
            "judge_invalid_output",
        ),
        (
            Behaviour::Json(400, json!({"error": {"code": 400}})),
            "extraction_rejected",
        ),
        (Behaviour::Json(401, json!({})), "extraction_auth"),
        (Behaviour::Json(429, json!({})), "extraction_rate_limited"),
        (Behaviour::Json(503, json!({})), "extraction_unavailable"),
        (
            Behaviour::Raw(200, vec![b' '; 70 * 1024]),
            "extraction_too_large",
        ),
        (
            Behaviour::Delay(10_500, Box::new(Behaviour::Json(200, json!({})))),
            "extraction_timeout",
        ),
        (Behaviour::Close, "extraction_unavailable"),
    ];
    for (b, want) in cases {
        let (r, _) = extract(b).await;
        assert_eq!(r.unwrap_err().code, want);
    }
}

async fn decider(b: Behaviour) -> (GeminiDecider, Seen) {
    let (base, seen) = server(b).await;
    (
        GeminiDecider {
            cfg: cfg(&base),
            client: copilot_providers::http_client(),
        },
        seen,
    )
}

#[tokio::test]
async fn decisions_contract_and_failures() {
    let mut opts = BTreeMap::new();
    opts.insert("B".to_string(), "x".to_string());
    opts.insert("C".to_string(), "y".to_string());
    let ch = json!({"answers": {"lunch_pick": {"type": "choice", "choice": "C", "probabilities": {"B": 0.3, "C": 0.7}, "confidence": 0.7}}});
    let (d, seen) = decider(Behaviour::Json(200, ch)).await;
    assert_eq!(
        d.choice("s".into(), opts.clone()).await.unwrap().choice,
        "C"
    );
    assert_eq!(seen.path.lock().unwrap()[0], "/v1beta/decisions");
    let body = seen.body.lock().unwrap()[0].clone();
    assert_eq!(body["model"], "gemini-decision-1");
    assert_eq!(body["questions"]["lunch_pick"]["type"], "choice");

    let cases: Vec<(Behaviour, &str)> = vec![
        (
            Behaviour::Json(
                200,
                json!({"answers": {"lunch_pick": {"type": "choice", "choice": "B", "probabilities": {"B": 0.3, "C": 0.7}, "confidence": 0.3}}}),
            ),
            "provider_invalid_output",
        ),
        (
            Behaviour::Json(200, json!({"answers": {}})),
            "provider_invalid_output",
        ),
        (
            Behaviour::Raw(200, b"nope".to_vec()),
            "provider_invalid_output",
        ),
        (
            Behaviour::Json(
                400,
                json!({"error": {"code": "safety", "message": "blocked"}}),
            ),
            "decision_rejected",
        ),
        (Behaviour::Json(503, json!({})), "decision_unavailable"),
        (
            Behaviour::Raw(200, vec![b' '; 40 * 1024]),
            "decision_too_large",
        ),
        (
            Behaviour::Delay(2_300, Box::new(Behaviour::Json(200, json!({})))),
            "decision_timeout",
        ),
    ];
    for (b, want) in cases {
        let (d, _) = decider(b).await;
        assert_eq!(
            d.choice("s".into(), opts.clone()).await.unwrap_err().code,
            want
        );
    }
}

#[tokio::test]
async fn slow_valid_extraction_within_ten_seconds_succeeds() {
    let out = json!({"outcome":"ASK","reason":"Trial?"}).to_string();
    let (r, _) = extract(Behaviour::Delay(
        7_500,
        Box::new(Behaviour::Json(200, candidate(&out, "STOP"))),
    ))
    .await;
    assert_eq!(r.unwrap().outcome, Outcome::Ask);
}

#[test]
fn provider_schemas_carry_no_array_bounds() {
    // array bounds can exceed Gemini's decoding limits; cardinality is enforced server-side
    let lunch = copilot_providers::extraction_body(
        &ExtractRequest {
            transcript: "x".into(),
            lunch_options: String::new(),
        },
        "LOW",
    )
    .to_string();
    let judge = copilot_providers::judge_body("g", "c", "LOW").to_string();
    for body in [lunch, judge] {
        assert!(!body.contains("maxItems") && !body.contains("minItems"));
    }
}
