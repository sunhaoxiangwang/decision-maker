//! Bridge demo: prints every transcript with its turn and bridge latency. The UI gets the
//! same stream at http://127.0.0.1:8787/events. The orchestrator calls the same
//! `next_transcript` / `show` / `publish`.
//!   cargo run -- --band <team band number>         live listening, no taps
//!   cargo run -- --typed                           TYPED input, no band
//!   add --cue to answer every final turn with an Ask cue (tests the LED path)
use decision_maker::bridge::{Bridge, Cue};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let band = args
        .iter()
        .position(|a| a == "--band")
        .map(|i| args.get(i + 1).and_then(|n| n.parse::<u16>().ok()).expect("--band takes the team's band number"));
    if band.is_none() && !args.iter().any(|a| a == "--typed") {
        eprintln!("usage: decision-maker --band <team band number> | --typed  [--cue]");
        std::process::exit(2);
    }
    let cue = args.iter().any(|a| a == "--cue");
    let label = if band.is_some() { "BAND" } else { "TYPED" };
    let bridge = Bridge::start(band).unwrap_or_else(|e| {
        eprintln!("bridge failed to start: {e}");
        std::process::exit(1)
    });
    while let Some(t) = bridge.next_transcript() {
        let queued = t.received.elapsed().as_micros();
        println!(
            "{label} turn {} speaker {:?} final={} ({queued} µs in bridge): {}",
            t.turn_id, t.speaker, t.is_final, t.text
        );
        if cue && t.is_final {
            let start = Instant::now();
            let shown = bridge.show(t.turn_id, Cue::Ask);
            println!("  cue Ask -> {shown:?} in {} ms", start.elapsed().as_millis());
        }
    }
}
