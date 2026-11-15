//! copilot-server binary: `serve [--mock]` and `check-providers`.

use copilot_server::app::{App, Config};
use copilot_server::{check_providers, providers, router};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_target(true).init();
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("serve");
    match cmd {
        "check-providers" => std::process::exit(check_providers().await),
        "serve" => {}
        other => {
            eprintln!("usage: copilot-server [serve [--mock] | check-providers] (got '{other}')");
            std::process::exit(2);
        }
    }
    let mock = args.iter().any(|a| a == "--mock")
        || std::env::var("COPILOT_PROVIDERS").as_deref() == Ok("mock");
    let (extractor, decider, label) = match providers(mock) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(2);
        }
    };
    let bind = std::env::var("COPILOT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let addr: std::net::SocketAddr = match bind.parse() {
        Ok(a) => a,
        Err(_) => {
            eprintln!("COPILOT_BIND must be an address like 127.0.0.1:8787");
            std::process::exit(2);
        }
    };
    if !addr.ip().is_loopback() {
        eprintln!("refusing to bind a non-loopback address ({addr}); LAN access is not supported");
        std::process::exit(2);
    }
    let bridge_token = std::env::var("COPILOT_BRIDGE_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty());
    let data_dir = std::env::var("COPILOT_DATA_DIR")
        .unwrap_or_else(|_| "data".into())
        .into();
    let app = App::new(
        Config {
            bind: bind.clone(),
            data_dir,
            bridge_token,
            providers_label: label.clone(),
            voice_goals_auto: std::env::var("COPILOT_VOICE_GOALS").as_deref() == Ok("auto"),
        },
        extractor,
        decider,
        None,
        None,
    );
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!(providers = %label, %bind, pid = std::process::id(), "copilot-server listening");
    println!(
        "copilot-server on http://{bind}/  providers={label}  pid={}",
        std::process::id()
    );
    axum::serve(listener, router(app)).await.expect("serve");
}
