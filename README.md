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
