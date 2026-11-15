# Vexo Conversation Copilot — Leo's app (Rust)

Checks what is said in a conversation against the user's confirmed goals and publishes a wrist cue: **PROCEED / PAUSE / ASK**, or a lunch **selection A / B / C**. Two demos share one Rust pipeline: mattress **negotiation** and **lunch** among three options.

This repository holds the companion UI, backend, Rust orchestration, Gemini adapters, rules, state, memory and the bridge contract. Cy's bridge (capture, Deepgram transcription, band connection, LEDs) is separate and talks to this app through [`docs/contract-v1.md`](docs/contract-v1.md).

## Status — read this first

| Item | Status |
|---|---|
| Rules, state machine, UI, bridge API, memory store | Implemented; covered by tests |
| Gemini extraction (`generateContent`) and Gemini Decisions adapters | Implemented to the documented contracts; **no live call has been made** (no `GEMINI_API_KEY` was available) |
| Mock providers (`--mock`) | Deterministic stand-ins, labelled **MOCK** in the UI and in every response (`providers: "mock"`, `decision_source: "mock_decisions"`) — not evidence of Gemini integration |
| Real band / hardware | **Not tested.** Cue colours are proposals until Cy verifies them on the band |
| Contract with Cy | **Proposed, not yet agreed** |
| Mastra, OpenAI memory summaries, Places | Not part of this implementation |
| Voice context update (negotiation only, branch `leo/voice-context-update`) | Implemented; covered by tests; one live run with real Gemini and **simulated** bridge input. Not tested with Cy's bridge, gestures or the band |

## Layout

```
crates/copilot-core       pure logic: extraction contracts, validation, cost accounting, decision tables,
                          session state machine, lunch rules, bridge reference model, limits
crates/copilot-providers  Gemini adapters (server-side), bounded HTTP, secret wrapper, mock providers
crates/copilot-server     UI (Rust-rendered HTML, no JavaScript), bridge API, memory actor; bins copilot-server, replay
scenarios/*.json          scripted demos (simulated input)
docs/contract-v1.md       bridge contract for Cy
```

## Run

Requirements: Rust 1.98.1 (pinned in `rust-toolchain.toml`), macOS (TLS uses the system Security framework via `native-tls`). No Python, Node or C build tools are needed.

```sh
export COPILOT_BRIDGE_TOKEN=...            # generate locally; share with Cy outside chat and the repo
# real providers (set the key in your own terminal; never commit or paste it):
export GEMINI_API_KEY=...
cargo run --release -p copilot-server --bin copilot-server -- check-providers
cargo run --release -p copilot-server --bin copilot-server -- serve
# or deterministic MOCK providers:
cargo run --release -p copilot-server --bin copilot-server -- serve --mock
```

Open `http://127.0.0.1:8787/`. The server binds loopback only and refuses other addresses.

Configuration: `GEMINI_EXTRACT_MODEL` (default `gemini-3.8-flash`), `GEMINI_EXTRACT_THINKING` (default `LOW`; `MINIMAL` is rejected for 3.8 Flash), `GEMINI_DECISION_MODEL` (must be `gemini-decision-1`), `COPILOT_DATA_DIR` (default `data/`, git-ignored), `COPILOT_BIND` (loopback only).

Without `GEMINI_API_KEY` and without `--mock`, the app runs with providers **unconfigured**: every turn returns `UNAVAILABLE` (`missing_credentials`) — it never fabricates a decision.

## Demo: negotiation (UI)

1. Start a **negotiation** session; confirm goals (queen, firm, ≤ $1,200 incl. delivery, ≥ 100 nights).
2. Type turns (labelled SIMULATED/typed) or let Cy's bridge send them:
   - `$1,399.` → **PAUSE** (red): over budget even with other terms missing.
   - `$1,150 delivered, with a 30-day return.` → **PAUSE**: return-policy conflict.
   - `Best I can do is $1,150 delivered, queen, firm.` → **ASK** (amber): return policy missing.
   - `Best I can do is $1,150 delivered, queen, firm, with a 100-night return.` → **ASK**, waiting for confirmation.
3. Open **/confirm** (link in the status panel), check the quoted evidence, **Confirm all shown** → **PROCEED** (green) after the contextual check.
4. Evidence is checked in its original clause: "it is not firm" can never become "firm". A rejected critical reading stays open as an *unresolved interpretation* and blocks PROCEED until you **correct the value** or **dismiss** it. Unclear turns can be ignored or explicitly treated as a new offer or an amendment. A failed confirmation-triggered evaluation shows **Retry evaluation**.

5. **Voice context update** (SIMULATED typed form, or the bridge after the three-wave gesture): `My budget is now $1,300 including delivery.` → a proposal shows current vs. proposed goals with the quoted evidence; cues stay at most ASK (PAUSE if already violated) until you **Confirm** (goals change, offer re-evaluated), **Retry** or **Discard** (goals unchanged). Relative changes are not proposed. Lunch does not support voice updates.

## Demo: lunch (UI)

1. Start a **lunch** session; add confirmed preferences (peanut allergy, max walk 1.0 mi, prefer vegetarian).
2. **Load demo options** (user-entered fixture: A Thai Basil, B Green Bowl, C Burger Barn) → **SELECT B** (purple) by rules: A contains peanuts, C is 1.8 mi.
3. Type `Burger Barn is 0.8 miles away.` → C stays excluded because its confirmed 1.8 mi conflicts (contradictions are kept, never overwritten). **Set fact** C distance 0.8 (an explicit user entry replaces that kind of fact) → B and C eligible → the Decisions provider picks.
4. Renaming an option removes its previous facts (a new restaurant starts empty).

## Scripted demos (SIMULATED bridge + simulated user)

```sh
cargo run --release -p copilot-server --bin copilot-server -- serve --mock      # terminal 1
COPILOT_BRIDGE_TOKEN=$COPILOT_BRIDGE_TOKEN cargo run --release -p copilot-server --bin replay -- --scenario scenarios/negotiation.json
COPILOT_BRIDGE_TOKEN=$COPILOT_BRIDGE_TOKEN cargo run --release -p copilot-server --bin replay -- --scenario scenarios/lunch.json
# against a server started WITHOUT a key and without --mock:
COPILOT_BRIDGE_TOKEN=$COPILOT_BRIDGE_TOKEN cargo run --release -p copilot-server --bin replay -- --scenario scenarios/failures.json
```

`replay` sends turns through the bridge API with `transcript_source: "simulated"`, confirms through the UI forms, and applies cues with the reference bridge model. It reports latency per generation — from the turn submission through the final applied cue, including confirmation waits (`pause_ms` simulates the user) — next to the server-side `e2e` / `human_wait` / `processing` timings. All of this is simulated, not hardware latency. `--stress-retention` fills the server to its retention limits and asserts the measured occupancy.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
cargo tree --locked -e normal,build -i aws-lc-sys   # expect: no match
cargo tree --locked -e normal,build -i ring         # expect: no match
cargo tree --locked -e normal,build -i openssl-sys  # expect: nothing on macOS
COPILOT_BRIDGE_TOKEN=x cargo run --release -p copilot-server --bin replay -- --stress-retention   # retention load (simulated)
```

## Safety and limits

- Keys live only in server environment variables, behind a redacting `Secret` type; they never reach the browser, logs or the repo.
- Transcript text is untrusted: it is escaped in HTML, never logged, and cannot change goals, rules or thresholds.
- Confidence never confirms a fact; only the user's explicit confirmation does.
- Limits: 64 requests in flight (including waiting polls), 8 waiting polls, 16 KiB bridge bodies, 4,000-character transcripts, 64 turn records and 1 MiB of transcripts per session, 1 active + 4 closed sessions.
- Contextual thresholds (PROCEED only when the Decisions "extra condition" probability ≤ 0.20; lunch selection needs ≥ 0.60) are **uncalibrated** defaults.
- Sessions do not survive a server restart; confirmed preferences do (`data/memory.json`, atomic writes; a corrupt file requires explicit recovery).
- Preference edits are bound to a monotonic publication number (not the reusable file version), so a form from before a reset or restore is rejected (409) without changes. A profile reset copies the bad file to a unique quarantine name, then atomically replaces `memory.json`; if the rename succeeded but the directory flush failed, the app publishes what is on disk, reports the save as uncertain and blocks lunch until **Re-check** succeeds. After a restart, a replaced file loads as the requested new profile; if a crash reverted it, the corrupt file is detected again and recovery is required.

---

# Decision Maker — speech integration

This branch connects band speech to the browser. Decision generation and the server on port 8788 are deferred to a separate component. Memory and goals are local to this browser; they are not acknowledged by a decision backend.

The current implementation embeds Python through PyO3. The Rust-only replacement is deferred. Rust owns event delivery and transcript handoff; the pinned kit handles Bluetooth and Deepgram.

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

Open **http://localhost:3000**, the bridge's allowed browser origin. Speech events come directly from `http://127.0.0.1:8787/events`. Keep this page open while recording. Audio is sent to Deepgram while the microphone is on; tell people nearby before recording.

- Triple-tap: conversation capture on/off. Green/red flashes acknowledge the change.
- Wave: record memory until the next wave. Finished transcript turns are saved as given.
- The mic is paused during activation flashes (2 s), wave acknowledgement (0.8 s), and optional demo cues (2 s). Wait for blue/the live mic indicator before speaking. These gaps are real missing audio, not buffered audio.
- `--cue` is only a fixed Ask LED demo; it does not generate decisions.

Without the kit, Bluetooth packages, keys or hardware, typed input still works (a compatible Python interpreter is required):

```sh
sh scripts/setup.sh --typed   # creates the Python environment and UI dependencies; no kit or Bluetooth installation
cargo run -- --typed
```

Enter `/triple`, transcript lines, `/wave`, more transcript lines, `/wave`, and `/triple`.

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

Mandatory constraints and user context are outside optional top-k retrieval. A future decision component must add acknowledged memory revisions, invalidate in-flight decisions when memory changes, and enforce its own mandatory-context request budget. There is no claim of backend memory sync on this branch.

## Verification

```sh
cargo test
cargo build
.venv/bin/python3 pipeline/test_band_pipeline.py
.venv/bin/python3 pipeline/test_bridge.py
.venv/bin/python3 pipeline/test_slow_client.py
npm --prefix web test
npm --prefix web run build
```

Socket tests bind localhost only. Python gesture tests use fake hardware; speech-adapter tests require the kit and explicitly skip when it is absent. The typed integration test uses actual Python → Rust → SSE → frontend reducer events and requires neither the kit nor network services. It checks transcript display/history, wave capture, idle HTTP clients and reconnect replay.
