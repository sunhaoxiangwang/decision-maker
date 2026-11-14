# Speech integration hardening report

Scope: harden the current speech-to-display integration on `frontend-integration`. Decision generation and a Rust-only rewrite were explicitly deferred by the user. Changes are in the working tree, not committed or pushed.

## Completed

- Moved SSE socket writes out of capture callbacks into independent writers, with at most 8 connections, 16 queued frames/client, 64-KiB events and 500-ms header/write deadlines. Full queues evict slow clients. Incomplete requests do not block accepting other subscribers.
- Added a 128-event / 1-MiB replay window and Last-Event-ID catch-up. Expired replay produces a visible history-gap notice. Mode, memory capture and actual mic status are sent to newly connected UIs.
- Bounded Rust handoff to 32 final transcripts, each at most 8 KiB. Interims stream to the browser without entering that backlog. Overflow stops capture and warns instead of blocking under Python's GIL.
- Fixed stale demo cues: flush pending speech, then revalidate turn, conversation and capture generation at the device operation under shared speech/mode locks. Publish success after the LED call succeeds. LED errors resume listening rather than leaving the mic paused indefinitely. This does not implement future decision-memory acknowledgement.
- Bounded speech accumulation by text/time, removed lifetime retention of final speech, and added a watchdog for stalled speech sends. Typed input and gesture checks no longer import the external speech kit eagerly.
- Kept mandatory constraints and user context outside optional top-k retrieval.
- Added browser history/memory byte/count budgets, disclosed retention and quota errors, batched persistence, preserved unreadable saved copies, and added history/memory export.
- Removed requests that silently attempted memory sync to the nonexistent decision server. Goals and memory are explicitly local; speech display works independently of goals.
- Bounded optional manual memory extraction by request/response bytes, stream-read deadlines, concurrency, fetch time and output tokens.
- Added pinned kit/Python setup instructions, `scripts/setup.sh` (including a kit-free `--typed` option), and project Python selection for Cargo. Existing kit checkouts are checked rather than overwritten.
- Preserved concurrent gesture-indicator UI edits in the shared working tree; final frontend checks cover the combined state.

## Validation

- Rust: 7 tests passed; binary build passed.
- Frontend: 23 tests passed; TypeScript and production build passed.
- Python assertions passed, including pending-speech stale-cue reproduction, LED failure recovery, bounded speech segmentation, consumer failure and speech-write watchdog.
- Actual typed Python → Rust → SSE → frontend reducer integration passed: conversation history, clearing, wave memory, reload, incomplete HTTP clients and reconnect replay.
- Non-reading SSE load regression passed: a healthy subscriber received all 800 large final turns and the final off event. This is synthetic localhost validation, not a radio-capacity or sponsor-throughput measurement.
- A temporary clean source copy without `vexo-open-kit` or `.venv` built offline using Python 3.12 and passed kit-free gesture and typed integration checks. Hardware setup's fresh network downloads were not rerun.
- `git diff --check` and setup shell syntax check passed.

## Remaining / deliberately deferred

- No decision service on port 8788, model-generated decisions or acknowledged backend memory revision protocol. The separate decision component must implement these and reject in-flight outputs from older memory revisions. `--cue` remains a fixed Ask demo.
- Embedded Python remains; Rust-only band/speech replacement is deferred.
- The microphone still loses input during LED holds: 2-s activation/demo cues and 0.8-s wave acknowledgement. Actual mic-state reporting now exposes these pauses.
- This hardening pass did not rerun a physical-band or live Deepgram test. Long RSS/radio/packet-loss/latency soak measurements remain undone. Retained-data budgets are not an operating-system RSS ceiling.
- Replay and history retention are finite. Keeping the UI closed throughout a recording cannot recover all speech later; expiry/pruning is disclosed.
