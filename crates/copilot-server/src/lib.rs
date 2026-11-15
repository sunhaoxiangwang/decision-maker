//! copilot-server library: router, app state, UI, bridge API, memory store.

pub mod api;
pub mod app;
pub mod memory;
pub mod ui;

use app::{App, Config};
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use copilot_core::limits::{BRIDGE_BODY_MAX, UI_FORM_MAX};
use copilot_providers::mock::{MockDecider, MockExtractor};
use copilot_providers::{
    Decider, Extractor, GeminiConfig, GeminiDecider, GeminiExtractor, Unconfigured,
};
use std::sync::Arc;

pub fn router(app: Arc<App>) -> Router {
    let bridge = Router::new()
        .route("/v1/sessions/active", get(api::get_active))
        .route("/v1/sessions", post(api::post_session))
        .route("/v1/sessions/{sid}/turns", post(api::post_turn))
        .route("/v1/sessions/{sid}/turns/{cid}", get(api::get_turn))
        .route(
            "/v1/sessions/{sid}/context-updates",
            post(api::post_context_update),
        )
        .route(
            "/v1/sessions/{sid}/context-updates/{uid}",
            get(api::get_context_update),
        )
        .route(
            "/v1/sessions/{sid}/turns/{cid}/cancel",
            post(api::post_cancel),
        )
        .route(
            "/v1/sessions/{sid}/turns/{cid}/retry",
            post(api::post_retry),
        )
        .route("/v1/sessions/{sid}/cue", get(api::get_cue))
        .route("/v1/sessions/{sid}/cue-reports", post(api::post_cue_report))
        .route("/v1/diagnostics/occupancy", get(api::get_occupancy))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::bridge_auth,
        ))
        .layer(DefaultBodyLimit::max(BRIDGE_BODY_MAX))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::canonical_errors,
        ));
    let ui = Router::new()
        .route("/", get(ui::index))
        .route("/confirm", get(ui::confirm_page))
        .route("/ui/status", get(ui::status))
        .route("/ui/session", post(ui::post_session))
        .route("/ui/close", post(ui::post_close))
        .route("/ui/goals", post(ui::post_goals))
        .route("/ui/turn", post(ui::post_turn))
        .route("/ui/turn/discard", post(ui::post_discard))
        .route("/ui/turn/retry", post(ui::post_retry))
        .route("/ui/ambiguous/resolve", post(ui::post_ambiguous))
        .route("/ui/eval/retry", post(ui::post_eval_retry))
        .route(
            "/ui/lunch/unresolved/dismiss",
            post(ui::post_lunch_unresolved),
        )
        .route("/static/app.css", get(ui::css))
        .route("/ui/lunch/fixture", post(ui::post_lunch_fixture))
        .route("/ui/lunch/setup", post(ui::post_lunch_setup))
        .route("/ui/lunch/fact", post(ui::post_lunch_fact))
        .route("/ui/lunch/confirm", post(ui::post_lunch_confirm))
        .route("/ui/prefs/add", post(ui::post_pref_add))
        .route("/ui/prefs/delete", post(ui::post_pref_delete))
        .route("/ui/memory/recheck", post(ui::post_memory_recheck))
        .route("/ui/memory/new_profile", post(ui::post_memory_new_profile))
        .layer(DefaultBodyLimit::max(UI_FORM_MAX));
    Router::new()
        .merge(bridge)
        .merge(ui)
        .fallback(api::fallback)
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::admission,
        ))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            api::host_guard,
        ))
        .with_state(app)
}

pub type Providers = (Arc<dyn Extractor>, Arc<dyn Decider>, String);

pub fn providers(mode_mock: bool) -> Result<Providers, String> {
    if mode_mock {
        return Ok((
            Arc::new(MockExtractor),
            Arc::new(MockDecider),
            "mock".into(),
        ));
    }
    match GeminiConfig::from_env()? {
        Some(cfg) => {
            let client = copilot_providers::http_client();
            Ok((
                Arc::new(GeminiExtractor {
                    cfg: cfg.clone(),
                    client: client.clone(),
                }),
                Arc::new(GeminiDecider { cfg, client }),
                "real".into(),
            ))
        }
        None => Ok((
            Arc::new(Unconfigured),
            Arc::new(Unconfigured),
            "unconfigured".into(),
        )),
    }
}

pub async fn check_providers() -> i32 {
    use copilot_core::contract::Outcome;
    let cfg = match GeminiConfig::from_env() {
        Ok(Some(c)) => c,
        Ok(None) => {
            println!("gemini-judge     FAIL  missing_credentials (GEMINI_API_KEY not set)");
            println!("gemini-decisions FAIL  missing_credentials (GEMINI_API_KEY not set)");
            return 2;
        }
        Err(e) => {
            println!("config FAIL {e}");
            return 2;
        }
    };
    let client = copilot_providers::http_client();
    let ex = GeminiExtractor {
        cfg: cfg.clone(),
        client: client.clone(),
    };
    let mut code = 0;
    // a clear over-budget offer must come back PAUSE
    let t0 = std::time::Instant::now();
    let r = ex
        .judge(
            "Spend at most $100.".into(),
            "speaker1: The price is $1,399.".into(),
        )
        .await;
    let ms = t0.elapsed().as_millis();
    match r {
        Ok(j) if j.outcome == Outcome::Pause => {
            println!("gemini-judge     PASS  {ms} ms  PAUSE ({})", j.reason)
        }
        Ok(j) => {
            println!(
                "gemini-judge     FAIL  {ms} ms  expected PAUSE, got {:?} ({})",
                j.outcome, j.reason
            );
            code = 1;
        }
        Err(e) => {
            println!("gemini-judge     FAIL  {ms} ms  {}", e.code);
            code = 1;
        }
    }
    let dec = GeminiDecider { cfg, client };
    let t0 = std::time::Instant::now();
    let mut opts = std::collections::BTreeMap::new();
    opts.insert(
        "B".to_string(),
        "Green Bowl: vegetarian options yes".to_string(),
    );
    opts.insert(
        "C".to_string(),
        "Burger Barn: vegetarian options no".to_string(),
    );
    match dec
        .choice("prefer vegetarian options (soft)".into(), opts)
        .await
    {
        Ok(a) => println!(
            "gemini-decisions PASS  {} ms  choice shape valid (choice {})",
            t0.elapsed().as_millis(),
            a.choice
        ),
        Err(e) => {
            println!(
                "gemini-decisions FAIL  {} ms  choice {}",
                t0.elapsed().as_millis(),
                e.code
            );
            code = 1;
        }
    }
    code
}

/// Bind 127.0.0.1:0 and serve in the background (tests and replay harness).
pub async fn spawn(
    extractor: Arc<dyn Extractor>,
    decider: Arc<dyn Decider>,
    label: &str,
    data_dir: std::path::PathBuf,
    bridge_token: Option<String>,
    fail: Option<memory::FailPoint>,
    gate: Option<memory::Gate>,
) -> (Arc<App>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let app = App::new(
        Config {
            bind: addr.clone(),
            data_dir,
            bridge_token,
            providers_label: label.into(),
        },
        extractor,
        decider,
        fail,
        gate,
    );
    let r = router(app.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, r).await;
    });
    (app, addr)
}
