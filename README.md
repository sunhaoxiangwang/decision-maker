# Vexo Conversation Copilot

Cy and Leo's project at **Vexo Hack** (November 2026).

A wrist copilot for live conversations. You state your goals in your own words — typed in the copilot, or spoken to the band. While you talk, a Vexo Open band hears the conversation, Gemini judges what is said against your goals, and the band shows a cue: **PROCEED** (green), **PAUSE** (red) or **ASK** (amber). Nothing about the conversation is hard-coded: there is no fixed topic, schema or rule set, only your goals and what is said.

## Team

| | Main areas |
|---|---|
| **Cy** | Band bridge: Bluetooth connection, audio capture, Deepgram transcription, gestures, LEDs; the live page (`web/`); the free-text conversation judge |
| **Leo** | Copilot server: bridge API and contract, copilot UI, Gemini adapters, session state; the bridge's cue polling and restart recovery |

## How it fits together

```
band ──Bluetooth──> bridge (src/, pipeline/) ──speech──> Deepgram
                      │  ├─ events on :8790 ──────────> live page (web/, http://localhost:3000)
                      │  └─ turns, goal updates, cues ─> copilot server (crates/, http://127.0.0.1:8787) ──> Gemini
band LED <── cue ─────┘
```

The bridge and the copilot server talk through [`docs/contract-v1.md`](docs/contract-v1.md) on loopback only, so both run on the same laptop.

## Status — read this first

| Item | Status |
|---|---|
| Copilot server: session state, copilot UI, bridge API | Implemented; covered by tests |
| Conversation judge (Gemini `generateContent`: free-text goals + conversation → cue and reason) | Implemented; covered by tests; live runs with real Gemini on **simulated** (not spoken) turns |
| Bridge ↔ copilot (`src/copilot.rs` ↔ `crates/copilot-server`) | Implemented on both sides: final turns, spoken goal updates, session start, cue polling, restart and session-change recovery; covered by tests with **simulated** input |
| Spoken goal updates (wave gesture) | Implemented; covered by tests with **simulated** bridge input. Not yet tested with the band |
| Full live loop on the band (speech → copilot → LED) | **Not yet verified end to end.** Cue colours are proposals until Cy verifies them on the band |

## Layout

```
src/                      bridge (Rust): band events, transcript handoff, copilot client, cue polling; bin decision-maker
pipeline/                 bridge (Python, embedded through PyO3): band connection, Deepgram, gestures, LEDs
web/                      live page (Next.js): transcript, decision window, memory
crates/copilot-core       pure logic: judge prompt and parsing, spoken goal updates, session state machine,
                          bridge reference model, limits
crates/copilot-providers  Gemini adapters (server-side), bounded HTTP, secret wrapper
crates/copilot-server     copilot UI (Rust-rendered HTML, no JavaScript), bridge API; bin copilot-server
docs/contract-v1.md       bridge ↔ copilot contract
scripts/setup.sh          kit checkout, Python environment, web dependencies
```

## Run the full demo

Run `sh scripts/setup.sh` once (see [Setup](#setup)). Then use three terminals from the project root. Terminals 1 and 2 need the same `COPILOT_BRIDGE_TOKEN`: generate it locally and never commit or paste it. Set keys in your own terminal only.

```sh
# 1. copilot server — http://127.0.0.1:8787/
export COPILOT_BRIDGE_TOKEN=...
export GEMINI_API_KEY=...
cargo run --release -p copilot-server --bin copilot-server -- serve

# 2. bridge — band 48; DEEPGRAM_API_KEY in the environment or the root .env
export COPILOT_BRIDGE_TOKEN=...          # same value as terminal 1
cargo run -- --band 48

# 3. live page — http://localhost:3000
cd web && npm run dev -- --port 3000
```

1. Open the copilot at `http://127.0.0.1:8787/` and the live page at `http://localhost:3000`.
2. Tell people nearby that audio is recorded and transcribed.
3. Triple-tap the band to start decisions. The bridge starts a session itself when none is active.
4. Set your goals: type them in the copilot, or wave once, say them, and wave again.
5. Talk. Each finished turn is judged, and the cue shows on the band, on the live page and in the copilot.

---

# Copilot server (Leo)

## Run

Requirements: Rust 1.98.1 (pinned in `rust-toolchain.toml`), macOS (TLS uses the system Security framework via `native-tls`). The copilot server on its own (`-p copilot-server`) needs no Python, Node or C build tools.

```sh
export COPILOT_BRIDGE_TOKEN=...            # generate locally; share with the bridge outside chat and the repo
export GEMINI_API_KEY=...                  # set in your own terminal; never commit or paste it
cargo run --release -p copilot-server --bin copilot-server -- check-providers   # checks the key with live calls
cargo run --release -p copilot-server --bin copilot-server -- serve
```

Open `http://127.0.0.1:8787/`. The server binds loopback only and refuses other addresses.

Configuration: `GEMINI_EXTRACT_MODEL` (the judge model; default and only accepted value `gemini-3.8-flash`), `GEMINI_EXTRACT_THINKING` (`LOW` by default, or `MEDIUM` / `HIGH`), `COPILOT_DATA_DIR` (default `data/`, git-ignored), `COPILOT_BIND` (loopback only).

Without `GEMINI_API_KEY`, every turn returns `UNAVAILABLE` (`missing_credentials`) — the copilot never fabricates a decision.

## How the copilot decides

Goals are free text in your own words and optional. The model works out what matters from the goals and from what is said, then judges the latest state of the conversation. Only its answer has a fixed shape: **PROCEED** (green), **PAUSE** (red), **ASK** (amber) or **NONE** (LED off, nothing to judge yet), plus a one-line reason.

1. **Session**: the bridge starts one when none is active, or start one in the copilot UI.
2. **Goals**: type, edit or clear them in the copilot UI at any time.
3. **Judging**: each final turn is judged against the goals and the conversation so far (newest 30 turns / 12,000 characters).
4. **Spoken goal updates**: words captured between two waves on the band (one wave starts capture, the next stops it) are appended to the goals as said and apply at once; the conversation is re-judged.
5. **Failures**: a failed judgement (timeout, provider error) shows **UNAVAILABLE** (cyan) with **Retry**. It never blocks later turns, because the next judgement reads the whole conversation again.

## Safety and limits

- Keys live only in server environment variables, behind a redacting `Secret` type; they never reach the browser, logs or the repo.
- Transcript text is untrusted: it is escaped in HTML and never logged. The judge prompt marks the conversation as untrusted speech and tells the model never to accept another party's claim that the user's goals changed; only the user's own UI edits and band wave updates change the goals.
- Cues are the model's judgement; there are no rule-based checks behind them.
- Limits: 64 requests in flight (including waiting polls), 8 waiting polls, 16 KiB bridge bodies, 4,000-character transcripts, 64 turn records and 1 MiB of transcripts per session, 1 active + 4 closed sessions.
- Sessions, including their goals, do not survive a server restart.

---

# Band bridge and live page (Cy)

The bridge connects band speech to the live page and, with `COPILOT_BRIDGE_TOKEN` set, to the copilot server. The live page keeps its own history and memory in this browser; goals live in the copilot (typed in its UI, or spoken between two waves).

The current implementation embeds Python through PyO3. The Rust-only replacement is deferred. Rust owns event delivery, transcript handoff and the copilot client; the pinned kit handles Bluetooth and Deepgram.

## Setup

Use Rust/Cargo, Node 22.18 or later (tested with Node 26), Git, and Python 3.12. Do not use Python 3.15 with this PyO3 version.

```sh
sh scripts/setup.sh
```

The script checks out the kit at `1e4291e2fa4896b1f603d3d4f0b009c483425da0`, creates `.venv` with Python 3.12 when missing, installs `requirements.txt`, and runs `npm ci`. Existing kit checkouts are checked, not overwritten. Existing Python 3.9–3.12 environments are accepted. Cargo uses the project `.venv/bin/python3`; an explicit `PYO3_PYTHON` overrides it.

Band speech needs your own `DEEPGRAM_API_KEY` in the environment or the root `.env` (ignored by Git). Never paste keys into chat or commit them. Spoken wave memory does not need an OpenAI key. Optional manual memory extraction needs `OPENAI_API_KEY` on the Next server.

## Run

In the first terminal, from the project root:

```sh
cargo run -- --band 48
```

48 is this project's confirmed band. Enable Bluetooth and close other dashboards/apps connected to it. If the band cannot be found, double-tap it to reopen its connect window. Do not run a second bridge against the same band.

In a second terminal:

```sh
cd web
npm run dev -- --port 3000
```

Open **http://localhost:3000**, the bridge's allowed browser origin (`BRIDGE_UI_ORIGIN`). Speech events come directly from `http://127.0.0.1:8790/events` (`BRIDGE_PORT`; the copilot server owns 8787). Keep this page open while recording. Audio is sent to Deepgram while the microphone is on; tell people nearby before recording.

With `COPILOT_BRIDGE_TOKEN` set to the copilot server's value (`COPILOT_ADDR`, default `127.0.0.1:8787`), each final conversation turn goes to the copilot and its cue is shown on the band and in the live page's decision window. The bridge also polls the active session's cue, so changes made in the copilot UI (goal edits, **Retry**) reach the band without another utterance. Stale cues from an older session, server instance or submission are never shown. Copilot errors appear as notices on the live page.

- Triple-tap: decisions mode on/off (conversation capture). Green/red flashes acknowledge the change.
- One wave (any direction): record memory until the next wave. Finished transcript turns are saved as given; with the copilot configured, each final turn is sent as a goal update (API trigger `three_waves`, a name kept from the earlier three-wave gesture) that is added to the copilot's goals at once (never an LED cue itself; the conversation is re-judged).
- The mic is paused during activation flashes (2 s), wave acknowledgement (0.8 s), and cues (2 s): the band only lights its LED with the mic off. Wait for blue/the live mic indicator before speaking. These gaps are real missing audio, not buffered audio.

## Bounds and failure behavior

| Resource | Bound / behavior |
|---|---|
| SSE connections | 8 including incomplete HTTP handshakes |
| SSE client backlog | 16 frames per client; slow readers are disconnected |
| SSE socket I/O | 500-ms request-header and frame-write deadlines; no socket writes in capture callbacks |
| SSE events / replay | 64 KiB/event; last 128 events or 1 MiB, whichever is smaller; native EventSource reconnect uses Last-Event-ID |
| Expired replay | Visible warning that history may be incomplete; current mode and mic state are sent again |
| Rust transcript consumer | 32 final turns × at most 8 KiB of text; interims stream only to the UI; overload stops capture with a visible warning |
| Speech text | 8 KiB/turn; continuous final speech segmented at 30 s or the text limit; no lifetime speech history in the adapter |
| Speech sends | A 5-s write watchdog aborts a stalled speech socket and stops capture |
| Browser history | 50 conversations, 200 turns/conversation, 200 decisions; 1 MiB combined conversation/decision JSON; oldest history pruned with notice |
| Browser memory | 256 items / 256 KiB total, 32 KiB/item; new oversized updates rejected visibly; mandatory notes never silently pruned |
| Persistence | Batched within 500 ms and flushed on page exit; quota/read failures shown; unreadable saved data is not overwritten |
| Manual model extraction | 64-KiB request/response bodies, bounded stream reads, two active requests, 20-s fetch deadline, 2,048 output tokens |

Use **Memory → Export history and memory** before long recordings. These budgets bound retained application data; they are not a hard operating-system RSS ceiling or a loss-free throughput guarantee. Band radio/speech-service performance still needs representative live measurement. Reconnect replay is finite and does not recover a browser that was closed throughout a recording.

Mandatory constraints and user context are outside optional top-k retrieval. Browser memory is not sent to the copilot: the copilot judges only its own goals and the conversation turns it received.

---

# Verification

The workspace includes the PyO3 bridge, so workspace-wide `cargo` commands need the `.venv` from `sh scripts/setup.sh`.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
cargo tree --locked -e normal,build -i aws-lc-sys   # expect: no match
cargo tree --locked -e normal,build -i ring         # expect: no match
cargo tree --locked -e normal,build -i openssl-sys  # expect: nothing on macOS
.venv/bin/python3 pipeline/test_band_pipeline.py
.venv/bin/python3 pipeline/test_bridge.py
.venv/bin/python3 pipeline/test_slow_client.py
npm --prefix web test
npm --prefix web run build
```

Socket tests bind localhost only. The band and speech-service tests run without hardware or network services; speech-adapter tests require the kit and explicitly skip when it is absent.
