//! Bridge demo: prints every transcript with its turn and bridge latency. The UI gets the
//! same stream at http://127.0.0.1:8790/events. With COPILOT_BRIDGE_TOKEN set, each
//! final conversation turn goes to the copilot server (docs/contract-v1.md) and its cue is shown. The orchestrator calls the same
//! `next_transcript` / `show` / `publish`.
//!   cargo run -- --band <team band number>         triple tap to listen
//!   cargo run -- --typed                           TYPED input, no band
//!   add --cue to answer every final turn with an Ask cue (tests the LED path)
use decision_maker::bridge::{Bridge, Cue, Purpose};
use decision_maker::copilot::{self, Copilot};
use decision_maker::cue_poll::{self, Step};
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
    let copilot = Copilot::from_env();
    if copilot.is_none() {
        eprintln!("COPILOT_BRIDGE_TOKEN not set: transcripts are not sent to the copilot server.");
    }
    // One cue model for polled cues and turn results: stale cues never reach the LED.
    let model = cue_poll::new_model();
    let cues = bridge.cue_handle();
    if let Some(poller) = Copilot::from_env() {
        cue_poll::spawn(poller, model.clone(), cues.clone());
    }
    while let Some(t) = bridge.next_transcript() {
        let queued = t.received.elapsed().as_micros();
        println!(
            "{label} conversation {} turn {} speaker {:?} final={} {:?} ({queued} µs in bridge): {}",
            t.conversation, t.turn_id, t.speaker, t.is_final, t.purpose, t.text
        );
        if let (Some(c), true) = (&copilot, t.is_final && t.purpose == Purpose::Conversation) {
            let submitted = c.client_turn_id(t.turn_id);
            cue_poll::capture(&mut model.lock().unwrap(), &submitted); // no cues until this ID's response
            cue_poll::apply(&cues, &model, "", &Step::LedOff("capture"), &serde_json::Value::Null);
            match c.submit(t.turn_id, &t.text, t.speaker) {
                Ok(result) => {
                    let snap = &result["snapshot"];
                    // validated first: a stale or foreign response reaches neither the UI nor the LED
                    let step = cue_poll::on_turn_result(&mut model.lock().unwrap(), &submitted, &result);
                    if !matches!(step, Step::Ignore(_)) {
                        if let Some(ev) = copilot::decision_event(t.turn_id, snap, now_ms()) {
                            let _ = bridge.publish(&ev); // UI only; the LED below does not depend on it
                        }
                    }
                    match (&step, copilot::cue_of(snap)) {
                        (Step::Apply { rev, .. }, Some(cue)) => {
                            println!("  decision received rev {rev}: copilot {}", snap["outcome"]);
                            let shown = bridge.show(&t, cue);
                            if let Ok(true) = shown {
                                cues.mark_lit(cue.led().is_some());
                            }
                            println!("  LED applied rev {rev}: {shown:?}");
                        }
                        (Step::Ignore(why), _) => println!("  copilot {} not applied: {why}", snap["outcome"]),
                        _ => {
                            let sid = model.lock().unwrap().session.clone().unwrap_or_default();
                            cue_poll::apply(&cues, &model, &sid, &step, snap)
                        }
                    }
                }
                Err(e) => eprintln!("  {e}"), // stays awaiting: fail closed until a newer turn is accepted
            }
        } else if let (Some(c), true) = (&copilot, t.is_final && t.purpose == Purpose::Context) {
            // goal update spoken between three-wave gestures: proposal only, never an LED cue
            if sends_three_waves_context(band.is_some()) {
                match c.context_update(t.turn_id, &t.text) {
                    Ok(r) => {
                        cue_poll::on_context_result(&mut model.lock().unwrap(), &r);
                        println!("  context update {} -> {} (confirm in the copilot UI)", r["update"]["update_id"], r["status"])
                    }
                    Err(e) => eprintln!("  {e}"),
                }
            } else {
                println!("  TYPED context not sent to the copilot: /wave only simulates the band's three-wave gesture");
            }
        } else if cue && t.is_final && t.purpose == Purpose::Conversation {
            let start = Instant::now();
            let shown = bridge.show(&t, Cue::Ask);
            println!("  cue Ask -> {shown:?} in {} ms", start.elapsed().as_millis());
        }
    }
}

/// Context updates go to the copilot with trigger `three_waves` (src/copilot.rs). That is only
/// true with the band, where the wrapper opens a context window solely on three recognised
/// waves (pipeline/band_pipeline.py THREE_WAVES); typed `/wave` is a simulation, so typed
/// context stays local (UI memory only) and is never submitted as a gesture.
fn sends_three_waves_context(band_mode: bool) -> bool {
    band_mode
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::sends_three_waves_context;

    #[test]
    fn only_band_context_is_labelled_three_waves() {
        assert!(sends_three_waves_context(true));
        assert!(!sends_three_waves_context(false)); // typed /wave never reaches /context-updates
    }
}
