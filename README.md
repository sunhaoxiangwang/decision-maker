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
