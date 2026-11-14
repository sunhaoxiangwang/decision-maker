//! Rust <-> Python bridge for the Vexo band pipeline.
//!
//! The Python pipeline (`pipeline/band_pipeline.py`) runs embedded in this process and
//! streams the band's microphone to Deepgram live. Transcripts cross into Rust through an
//! mpsc channel (for the orchestrator) and are broadcast as Server-Sent Events (for the UI).
//! LED cues go back as a direct Python call.
//! ponytail: in-process (PyO3) ties the build to one Python install; switch to a Unix
//! socket with JSON lines (~30 µs/msg) if that gets in the way.
use pyo3::prelude::*;
use pyo3::types::{PyCFunction, PyDict, PyTuple};
use crate::events::Hub;
#[cfg(test)]
use crate::events::allowed;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::{sync_channel, Receiver, TrySendError};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Exactly what the pipeline produced, plus the turn it belongs to.
/// A turn ends at each pause Deepgram hears. Interim text (`is_final == false`) carries the
/// turn still being spoken.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub turn_id: u64,
    /// Which decisions-mode session this belongs to: goes up by one each time the user
    /// triple-taps decisions mode on. A new number = a new conversation (and Deepgram's
    /// speaker numbers start again).
    pub conversation: u64,
    /// Deepgram's diarization label, 0-based.
    pub speaker: Option<u32>,
    pub text: String,
    pub is_final: bool,
    /// Context = speech captured between waves, saved verbatim as memory; never decide on it.
    pub purpose: Purpose,
    /// When the bridge received it from Python (for latency measurement).
    pub received: Instant,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Purpose {
    Conversation,
    Context,
}

impl Purpose {
    fn from_py(s: Option<&str>) -> Purpose {
        if s == Some("context") { Purpose::Context } else { Purpose::Conversation }
    }
    fn as_str(self) -> &'static str {
        match self {
            Purpose::Conversation => "conversation",
            Purpose::Context => "context",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cue {
    Proceed,
    Pause,
    Ask,
    /// Lunch option 'A', 'B' or 'C'. Anything else = no option selected (light off).
    Lunch(char),
    Off,
}

impl Cue {
    /// (hex colour, pattern) for the band; None = light off.
    pub fn led(self) -> Option<(&'static str, &'static str)> {
        match self {
            Cue::Proceed => Some(("#00ff00", "solid")),
            Cue::Pause => Some(("#ff0000", "blink")),
            Cue::Ask => Some(("#ffbf00", "breathe")),
            Cue::Lunch('A') => Some(("#00ff00", "solid")),
            Cue::Lunch('B') => Some(("#aa00ff", "solid")),
            Cue::Lunch('C') => Some(("#ffffff", "solid")),
            Cue::Lunch(_) | Cue::Off => None,
        }
    }
}

/// Last finished turn. Interim text belongs to the next one.
#[derive(Default)]
struct Turns(AtomicU64);

impl Turns {
    fn assign(&self, is_final: bool) -> u64 {
        if is_final {
            self.0.fetch_add(1, SeqCst) + 1
        } else {
            self.0.load(SeqCst) + 1
        }
    }
    fn is_latest(&self, turn_id: u64) -> bool {
        self.0.load(SeqCst) == turn_id
    }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

fn transcript_json(t: &Transcript) -> String {
    let speaker = t.speaker.map_or("null".to_string(), |s| s.to_string());
    format!(
        r#"{{"type":"transcript","turn_id":{},"conversation":{},"speaker":{speaker},"text":{},"final":{},"purpose":"{}","time_ms":{}}}"#,
        t.turn_id,
        t.conversation,
        json_str(&t.text),
        t.is_final,
        t.purpose.as_str(),
        now_ms()
    )
}

pub struct Bridge {
    decisions: Arc<AtomicBool>,
    conversation: Arc<AtomicU64>,
    memory: Arc<AtomicBool>,
    decision_turn: Arc<AtomicU64>,
    rx: Receiver<Transcript>,
    turns: Arc<Turns>,
    hub: Arc<Hub>,
    pipeline: Py<PyModule>,
    generation: Arc<AtomicU64>,
}

impl Bridge {
    /// `band`: the team's band number (live listening), or None for TYPED input from stdin.
    /// The UI event stream listens on 127.0.0.1:`BRIDGE_PORT` (default 8787) and answers
    /// browsers only from `BRIDGE_UI_ORIGIN` (default http://localhost:3000, Next.js dev).
    pub fn start(band: Option<u16>) -> Result<Bridge, String> {
        let port = std::env::var("BRIDGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
        let ui_origin = std::env::var("BRIDGE_UI_ORIGIN").unwrap_or_else(|_| "http://localhost:3000".into());
        let hub = Arc::new(Hub::default());
        hub.send(&format!(r#"{{"type":"mode","decisions":false,"conversation":0,"time_ms":{}}}"#, now_ms()));
        hub.send(&format!(r#"{{"type":"gesture","name":"wave","memory":false,"time_ms":{}}}"#, now_ms()));
        hub.send(r#"{"type":"status","band":"off","mic":"off"}"#);
        hub.clone().serve(port, ui_origin).map_err(|e| format!("UI events port {port}: {e}"))?;

        let (tx, rx) = sync_channel(32);
        let turns = Arc::new(Turns::default());
        let (emit_turns, emit_hub, publish_hub, mode_hub) = (turns.clone(), hub.clone(), hub.clone(), hub.clone());
        let decisions = Arc::new(AtomicBool::new(false)); // off until the user triple-taps
        let memory = Arc::new(AtomicBool::new(false));
        let decision_turn = Arc::new(AtomicU64::new(0));
        let generation = Arc::new(AtomicU64::new(now_ms() as u64));
        let (emit_generation, mode_generation, memory_generation) = (generation.clone(), generation.clone(), generation.clone());
        let (emit_on, emit_memory, emit_decision_turn) = (decisions.clone(), memory.clone(), decision_turn.clone());
        let (memory_on, memory_hub, memory_turn) = (memory.clone(), hub.clone(), decision_turn.clone());
        let mode_turn = decision_turn.clone();
        let conversation = Arc::new(AtomicU64::new(0));
        let (mode_on, mode_conv, emit_conv) = (decisions.clone(), conversation.clone(), conversation.clone());
        let pipeline = Python::with_gil(|py| -> PyResult<Py<PyModule>> {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/pipeline");
            py.import_bound("site")?.call_method1("addsitedir", (env!("BRIDGE_SITE_PACKAGES"),))?; // venv packages (bleak)
            py.import_bound("sys")?.getattr("path")?.call_method1("insert", (0, dir))?;
            let m = py.import_bound("band_pipeline")?;
            let emit = PyCFunction::new_closure_bound(
                py,
                None,
                None,
                move |args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<bool> {
                    let (text, speaker, is_final, purpose): (String, Option<u32>, bool, Option<String>) =
                        if args.len() == 3 {
                            let (a, b, c) = args.extract()?;
                            (a, b, c, None)
                        } else {
                            args.extract()?
                        };
                    if text.len() > 8192 {
                        emit_hub.send(r#"{"type":"gap","message":"Transcript exceeded the byte limit; capture stopped."}"#);
                        return Ok(false);
                    }
                    let purpose = Purpose::from_py(purpose.as_deref());
                    if (purpose == Purpose::Conversation && (!emit_on.load(SeqCst) || emit_memory.load(SeqCst)))
                        || (purpose == Purpose::Context && !emit_memory.load(SeqCst)) {
                        return Ok(true);
                    }
                    let t = Transcript {
                        turn_id: emit_turns.assign(is_final),
                        conversation: emit_conv.load(SeqCst),
                        speaker,
                        text,
                        is_final,
                        purpose,
                        received: Instant::now(),
                        generation: emit_generation.load(SeqCst),
                    };
                    if is_final && purpose == Purpose::Conversation {
                        emit_decision_turn.store(t.turn_id, SeqCst);
                    }
                    let json = transcript_json(&t);
                    if is_final {
                        match tx.try_send(t) {
                            Ok(()) => (),
                            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                                emit_on.store(false, SeqCst);
                                emit_decision_turn.store(0, SeqCst);
                                emit_hub.send(r#"{"type":"gap","message":"Transcript consumer is full; capture stopped. Restart after the consumer catches up."}"#);
                                return Ok(false);
                            }
                        }
                    }
                    emit_hub.send(&json);
                    Ok(true)
                },
            )?;
            m.setattr("emit", emit)?;
            // Additional Python UI events, encoded as JSON.
            let publish = PyCFunction::new_closure_bound(
                py,
                None,
                None,
                move |args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
                    let (json,): (String,) = args.extract()?;
                    if !publish_hub.send(&json) {
                        return Err(pyo3::exceptions::PyValueError::new_err("UI event exceeds 64 KiB"));
                    }
                    Ok(())
                },
            )?;
            m.setattr("publish", publish)?;
            // The handoff after Python saw a triple tap: decisions mode on or off.
            let set_mode = PyCFunction::new_closure_bound(
                py,
                None,
                None,
                move |args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
                    let (on,): (bool,) = args.extract()?;
                    let n = if on { mode_conv.fetch_add(1, SeqCst) + 1 } else { mode_conv.load(SeqCst) };
                    mode_on.store(on, SeqCst);
                    mode_generation.fetch_add(1, SeqCst);
                    let json = format!(r#"{{"type":"mode","decisions":{on},"conversation":{n},"time_ms":{}}}"#, now_ms());
                    mode_turn.store(0, SeqCst); // no cues from a previous conversation
                    mode_hub.send(&json);
                    Ok(())
                },
            )?;
            m.setattr("set_mode", set_mode)?;
            let set_memory_mode = PyCFunction::new_closure_bound(
                py, None, None,
                move |args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
                    let (on,): (bool,) = args.extract()?;
                    memory_on.store(on, SeqCst);
                    memory_generation.fetch_add(1, SeqCst);
                    memory_turn.store(0, SeqCst);
                    let json = format!(r#"{{"type":"gesture","name":"wave","memory":{on},"time_ms":{}}}"#, now_ms());
                    memory_hub.send(&json);
                    Ok(())
                },
            )?;
            m.setattr("set_memory_mode", set_memory_mode)?;
            Ok(m.unbind())
        })
        .map_err(|e| e.to_string())?;

        let runner = Python::with_gil(|py| pipeline.clone_ref(py));
        std::thread::spawn(move || {
            Python::with_gil(|py| {
                let m = runner.bind(py);
                if let Err(e) = m.call_method1("run", (band,)) {
                    e.print(py);
                }
                let _ = m.delattr("emit"); // drops the sender: next_transcript() returns None
            })
        });
        Ok(Bridge { decisions, memory, decision_turn, rx, turns, hub, pipeline, generation, conversation })
    }

    /// Blocks for the next final transcript. Interims go only to the UI, avoiding a
    /// decision backlog of superseded guesses. Capture stops if 32 finals accumulate.
    pub fn next_transcript(&self) -> Option<Transcript> {
        self.rx.recv().ok()
    }

    /// Shows `cue` for the latest conversation final. Off, memory capture, stale turns and
    /// mode transitions return Ok(false). The mic pauses for the light, then resumes.
    pub fn show(&self, transcript: &Transcript, cue: Cue) -> Result<bool, String> {
        let turn_id = transcript.turn_id;
        let expected_generation = transcript.generation;
        let expected_conversation = transcript.conversation;
        let (on, memory, latest, decision, generation, conversation) = (
            self.decisions.clone(), self.memory.clone(), self.turns.clone(),
            self.decision_turn.clone(), self.generation.clone(), self.conversation.clone(),
        );
        let shown = Python::with_gil(|py| -> Result<bool, String> {
            if !transcript.is_final || transcript.purpose != Purpose::Conversation
                || turn_id == 0 || !self.decisions_on() || self.memory_on()
                || !self.turns.is_latest(turn_id) || self.decision_turn.load(SeqCst) != turn_id
                || self.generation.load(SeqCst) != expected_generation
                || self.conversation.load(SeqCst) != expected_conversation {
                return Ok(false);
            }
            // Python invokes this AFTER flushing pending speech and disabling the mic,
            // while its mode and speech locks exclude all competing device operations.
            let validate = PyCFunction::new_closure_bound(py, None, None,
                move |_args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<bool> {
                    Ok(turn_id != 0 && on.load(SeqCst) && !memory.load(SeqCst)
                        && latest.is_latest(turn_id) && decision.load(SeqCst) == turn_id
                        && generation.load(SeqCst) == expected_generation
                        && conversation.load(SeqCst) == expected_conversation)
                }).map_err(|e| e.to_string())?;
            let (hex, pattern) = cue.led().map_or((None, "solid"), |(h, p)| (Some(h), p));
            self.pipeline.bind(py).call_method1("set_cue", (hex, pattern, validate))
                .and_then(|result| result.extract::<bool>()).map_err(|e| e.to_string())
        })?;
        self.hub.send(&format!(
            r#"{{"type":"cue","turn_id":{turn_id},"cue":{},"shown":{shown},"time_ms":{}}}"#,
            json_str(&format!("{cue:?}")),
            now_ms()
        ));
        Ok(shown)
    }

    /// True while decisions mode is on (triple tap). Memory capture may temporarily suspend cues.
    pub fn decisions_on(&self) -> bool {
        self.decisions.load(SeqCst)
    }

    /// True between waves, including when decisions mode is off. Speech is context only.
    pub fn memory_on(&self) -> bool {
        self.memory.load(SeqCst)
    }

    /// Sends the orchestrator's JSON (with a "type" field) to the UI event stream.
    #[must_use]
    pub fn publish(&self, json: &str) -> bool {
        self.hub.send(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_number_finals_and_drop_stale() {
        let t = Turns::default();
        assert_eq!(t.assign(false), 1); // interim of turn 1
        assert_eq!(t.assign(true), 1);
        assert!(t.is_latest(1));
        assert_eq!(t.assign(false), 2);
        assert!(t.is_latest(1)); // interim doesn't finish a turn
        assert_eq!(t.assign(true), 2);
        assert!(!t.is_latest(1)); // turn 1 is now stale
        assert!(t.is_latest(2));
    }

    #[test]
    fn cue_colours() {
        assert_eq!(Cue::Pause.led(), Some(("#ff0000", "blink")));
        assert_eq!(Cue::Ask.led().unwrap().0, "#ffbf00");
        assert_eq!(Cue::Lunch('B').led().unwrap().0, "#aa00ff");
        assert_eq!(Cue::Lunch('C').led().unwrap().0, "#ffffff");
        assert_eq!(Cue::Lunch('D').led(), None);
        assert_eq!(Cue::Off.led(), None);
    }

    #[test]
    fn only_the_ui_origin_on_localhost() {
        let ui = "http://localhost:3000";
        let req = |host: &str, origin: &str| format!("GET /events HTTP/1.1\r\nHost: {host}\r\n{origin}\r\n");
        assert!(allowed(&req("127.0.0.1:8787", "Origin: http://localhost:3000"), 8787, ui));
        assert!(allowed(&req("localhost:8787", ""), 8787, ui)); // curl, no Origin
        assert!(!allowed(&req("127.0.0.1:8787", "Origin: https://evil.example"), 8787, ui));
        assert!(!allowed(&req("evil.example:8787", ""), 8787, ui)); // DNS rebinding
        assert!(!allowed("GET / HTTP/1.1\r\n\r\n", 8787, ui)); // no Host
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_str("say \"$1,150\"\n\\ok\u{1}"), r#""say \"$1,150\"\n\\ok\u0001""#);
    }

    #[test]
    fn cues_require_a_current_conversation_turn_and_both_layers_accepting() {
        let pipeline = Python::with_gil(|py| {
            PyModule::from_code_bound(py, "accept = True\ndef set_cue(hex, pattern, valid):\n    return accept and valid()\n", "cue_test.py", "cue_test")
                .unwrap().unbind()
        });
        let (_, rx) = sync_channel(32);
        let b = Bridge {
            decisions: Arc::new(AtomicBool::new(false)),
            conversation: Arc::new(AtomicU64::new(1)),
            memory: Arc::new(AtomicBool::new(false)),
            decision_turn: Arc::new(AtomicU64::new(1)),
            rx, turns: Arc::new(Turns::default()), hub: Arc::new(Hub::default()), pipeline,
            generation: Arc::new(AtomicU64::new(1)),
        };
        let transcript = |turn_id| Transcript { turn_id, conversation: 1, speaker: None,
            text: "test".into(), is_final: true, purpose: Purpose::Conversation,
            received: Instant::now(), generation: 1 };
        b.turns.assign(true);
        assert!(!b.show(&transcript(1), Cue::Ask).unwrap()); // decisions start off
        b.decisions.store(true, SeqCst);
        assert!(!b.show(&transcript(0), Cue::Ask).unwrap()); // no transcript has turn zero
        assert!(b.show(&transcript(1), Cue::Ask).unwrap());
        b.memory.store(true, SeqCst);
        assert!(!b.show(&transcript(1), Cue::Ask).unwrap()); // waves suspend decision cues
        b.memory.store(false, SeqCst);
        b.decision_turn.store(0, SeqCst);
        assert!(!b.show(&transcript(1), Cue::Ask).unwrap()); // previous session or capture boundary
        b.turns.assign(true);
        assert!(!b.show(&transcript(2), Cue::Ask).unwrap()); // context can never trigger a cue
        b.decision_turn.store(2, SeqCst);
        assert!(!b.show(&transcript(1), Cue::Ask).unwrap()); // stale conversation turn
        Python::with_gil(|py| b.pipeline.bind(py).setattr("accept", false).unwrap());
        assert!(!b.show(&transcript(2), Cue::Ask).unwrap()); // Python transitioning: never claim shown
        Python::with_gil(|py| b.pipeline.bind(py).setattr("accept", true).unwrap());
        assert!(b.show(&transcript(2), Cue::Ask).unwrap());
        b.generation.store(2, SeqCst);
        assert!(!b.show(&transcript(2), Cue::Ask).unwrap());
        b.generation.store(1, SeqCst);
        b.conversation.store(2, SeqCst);
        assert!(!b.show(&transcript(2), Cue::Ask).unwrap());
        b.conversation.store(1, SeqCst);
        Python::with_gil(|py| {
            let latest = b.turns.clone();
            let invalidate = PyCFunction::new_closure_bound(py, None, None,
                move |_args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
                    latest.assign(true);
                    Ok(())
                }).unwrap();
            b.pipeline.bind(py).setattr("invalidate", invalidate).unwrap();
            py.run_bound("def set_cue(hex, pattern, valid):\n    invalidate()\n    return valid()\n", Some(&b.pipeline.bind(py).dict()), None).unwrap();
        });
        assert!(!b.show(&transcript(2), Cue::Ask).unwrap()); // flush emitted a newer final
    }
}
