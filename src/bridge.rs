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
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Exactly what the pipeline produced, plus the turn it belongs to.
/// A turn ends at each pause Deepgram hears. Interim text (`is_final == false`) carries the
/// turn still being spoken.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub turn_id: u64,
    /// Deepgram's diarization label, 0-based.
    pub speaker: Option<u32>,
    pub text: String,
    pub is_final: bool,
    /// When the bridge received it from Python (for latency measurement).
    pub received: Instant,
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

/// Server-Sent Events for the UI: `new EventSource("http://127.0.0.1:8787/events")`.
/// Every event is one JSON object with a "type" field.
#[derive(Default)]
struct Hub(Mutex<Vec<TcpStream>>);

impl Hub {
    /// Transcripts are private conversation: only `ui_origin` (the Next.js app) may read them
    /// from a browser, and only through a localhost Host name (blocks DNS rebinding).
    fn serve(self: Arc<Self>, port: u16, ui_origin: String) -> std::io::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", port))?; // this computer only
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                if !allowed(&request, port, &ui_origin) {
                    let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
                    continue;
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\
                     Access-Control-Allow-Origin: {ui_origin}\r\nVary: Origin\r\nConnection: keep-alive\r\n\r\n"
                );
                if stream.write_all(head.as_bytes()).is_ok() {
                    self.0.lock().unwrap().push(stream);
                }
            }
        });
        Ok(())
    }

    fn send(&self, json: &str) {
        let frame = format!("data: {json}\n\n");
        self.0.lock().unwrap().retain_mut(|s| s.write_all(frame.as_bytes()).is_ok());
    }
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Host must be localhost on our port; a browser's Origin must be the UI's. Requests with
/// no Origin (curl, the orchestrator) come from this computer and are allowed.
fn allowed(request: &str, port: u16, ui_origin: &str) -> bool {
    let host_ok = header(request, "host")
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    host_ok && header(request, "origin").map_or(true, |o| o == ui_origin)
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
        r#"{{"type":"transcript","turn_id":{},"speaker":{speaker},"text":{},"final":{},"time_ms":{}}}"#,
        t.turn_id,
        json_str(&t.text),
        t.is_final,
        now_ms()
    )
}

pub struct Bridge {
    rx: Receiver<Transcript>,
    turns: Arc<Turns>,
    hub: Arc<Hub>,
    pipeline: Py<PyModule>,
}

impl Bridge {
    /// `band`: the team's band number (live listening), or None for TYPED input from stdin.
    /// The UI event stream listens on 127.0.0.1:`BRIDGE_PORT` (default 8787) and answers
    /// browsers only from `BRIDGE_UI_ORIGIN` (default http://localhost:3000, Next.js dev).
    pub fn start(band: Option<u16>) -> Result<Bridge, String> {
        let port = std::env::var("BRIDGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
        let ui_origin = std::env::var("BRIDGE_UI_ORIGIN").unwrap_or_else(|_| "http://localhost:3000".into());
        let hub = Arc::new(Hub::default());
        hub.clone().serve(port, ui_origin).map_err(|e| format!("UI events port {port}: {e}"))?;

        let (tx, rx) = channel();
        let turns = Arc::new(Turns::default());
        let (emit_turns, emit_hub) = (turns.clone(), hub.clone());
        let pipeline = Python::with_gil(|py| -> PyResult<Py<PyModule>> {
            let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/pipeline");
            py.import_bound("site")?.call_method1("addsitedir", (env!("BRIDGE_SITE_PACKAGES"),))?; // venv packages (bleak)
            py.import_bound("sys")?.getattr("path")?.call_method1("insert", (0, dir))?;
            let m = py.import_bound("band_pipeline")?;
            let emit = PyCFunction::new_closure_bound(
                py,
                None,
                None,
                move |args: &Bound<'_, PyTuple>, _kw: Option<&Bound<'_, PyDict>>| -> PyResult<()> {
                    let (text, speaker, is_final): (String, Option<u32>, bool) = args.extract()?;
                    let t = Transcript {
                        turn_id: emit_turns.assign(is_final),
                        speaker,
                        text,
                        is_final,
                        received: Instant::now(),
                    };
                    emit_hub.send(&transcript_json(&t));
                    let _ = tx.send(t);
                    Ok(())
                },
            )?;
            m.setattr("emit", emit)?;
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
        Ok(Bridge { rx, turns, hub, pipeline })
    }

    /// Blocks for the next transcript (interim and final), passed on as the pipeline
    /// produced it. None once the pipeline has ended.
    pub fn next_transcript(&self) -> Option<Transcript> {
        self.rx.recv().ok()
    }

    /// Shows `cue` on the band for `turn_id`. Ok(false) = stale turn, dropped without
    /// touching the band. The mic pauses while the light shows, then resumes by itself.
    pub fn show(&self, turn_id: u64, cue: Cue) -> Result<bool, String> {
        let shown = self.turns.is_latest(turn_id);
        self.hub.send(&format!(
            r#"{{"type":"cue","turn_id":{turn_id},"cue":{},"shown":{shown},"time_ms":{}}}"#,
            json_str(&format!("{cue:?}")),
            now_ms()
        ));
        if !shown {
            return Ok(false);
        }
        let (hex, pattern) = match cue.led() {
            Some((hex, pattern)) => (Some(hex), pattern),
            None => (None, "solid"),
        };
        Python::with_gil(|py| {
            self.pipeline
                .bind(py)
                .call_method1("set_cue", (hex, pattern))
                .map(|_| true)
                .map_err(|e| e.to_string())
        })
    }

    /// Sends any JSON object of the orchestrator's (decision reasons, memory updates) to the
    /// UI on the same event stream. Give it a "type" field.
    pub fn publish(&self, json: &str) {
        self.hub.send(json);
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
}
