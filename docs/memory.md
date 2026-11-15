# Memory: how it's built, stored and fed to decisions

Memory holds verbatim wave transcripts and extracted sentence notes, with where each came
from. It is kept in the companion UI (the browser's localStorage) and sent to the proposed
orchestrator endpoint once that separate component exists. This branch saves memory locally only.

## Where notes come from

| Source | How | Status |
|---|---|---|
| **Speech between waves** | Saved as one confirmed context item per capture, with each final transcript turn on its own line. No model call or confirmation. | `confirmed`: the user recorded it |
| **Context the user types** (Memory tab, "Remember") | Sent to `POST /llm/memory` with `{source:"user", text}`. OpenAI splits it into notes. With no OpenAI key, each line is saved as it was typed. | `confirmed`: the user said it |
| **Past conversations** (Memory tab → Past conversations → "Remember this conversation") | The saved transcript is sent as lines (`t3 Seller: I can do $1,150`) to `POST /llm/memory` with `{source:"conversation", conversation_id, transcript}` | `proposed`: never used until the user confirms it |

Conversation history is saved while **decisions mode** is on; wave capture is stored separately as memory:
- **Triple-tap** the band (3 taps within 1.2 s; a `double-tap` counts as 2).
  - The band blinks green 5 times. A fresh Deepgram connection opens, so speaker numbers start again; the mic turns on after the blinks.
  - Rust gets the handoff (`set_mode(true)`) and every UI receives `{"type":"mode","decisions":true,"conversation":N}`. The UI opens a new conversation and clears the screen.
  - Every finished turn carries `conversation: N`.
- **Triple-tap again**: the band blinks red 5 times and the mic stops.
  - Words already spoken are still sent.
  - After that, `Bridge::show` sends no cues to the band. The mic stays off unless a wave starts memory capture.
  - The UI closes the conversation, clears the live chat and selected decision, and drops the conversation if nobody spoke. Finished turns remain in Memory → Past conversations.
- The bridge starts with the mode **off**. A UI that connects later receives the current mode straight away.
- **New conversation** on the Live tab, or a bridge restart, also starts a new conversation.
- Detection happens in Python (`TapCounter` in `pipeline/band_pipeline.py`).
- Before the demo, run once with `VEXO_GESTURE_DEBUG=1` to see which tap events the band sends for three taps. If a double-tap also sends separate taps, set `DOUBLE_TAP_COUNTS = 0`.
- The sensitivity is `TAP_THRESHOLD_G`.
- In typed mode, `/triple` acts out the triple tap.

`/llm/memory` runs on the Next.js server (`web/app/llm/memory/route.ts`):
- The key: it reads `OPENAI_API_KEY` from the environment or from `web/.env.local`, which is gitignored. The key never reaches the browser.
- The model: `OPENAI_MODEL` (default `gpt-6-luna`, the kit's model).
- Access: `web/proxy.ts` accepts only this UI's own JSON POSTs.

## Note format (`MemoryItem`, `web/lib/store.ts`)

```json
{
  "id": "6c1f…", "status": "confirmed",
  "kind": "constraint",
  "subject": "user",
  "text": "The user's budget is at most $1,200 including delivery.",
  "keywords": ["budget", "1200", "delivery"],
  "source": { "type": "conversation", "conversation_id": "c1794…-1-0", "turn_ids": [3] },
  "saved_ms": 1794690201912
}
```
- `kind`: `constraint` (hard limits, required terms), `preference`, `commitment` (what someone agreed to, with exact terms), `fact`, or `context`.
- `subject`: the speaker's name as set on the Live tab, `Speaker N`, or `user`.
  - Deepgram speaker numbers are only stable within one speech connection; mode and wave boundaries open fresh connections.
  - Name the speakers before pressing "Remember", so notes are about "Seller" rather than "Speaker 2".

## Retrieval (`web/lib/memory.ts`)

`retrieve(items, turnText, { subject })` returns all mandatory constraints/user context plus at most 8 optional matches, ranked together. Only confirmed notes are considered.
- Scoring:
  - Each of the note's keywords that the turn mentions: +2.
  - Any other word of the note that the turn mentions: +1.
  - The note is about the person speaking: +1.
- Constraints and notes the user typed are always included. A decision must never forget a goal.
- Matching ignores `$` and commas in numbers, so "$1,200" matches "1200".

## What the decision API receives (`decisionRequest`)

```json
{
  "turn":   { "turn_id": 4, "speaker": "Seller", "text": "OK, $1,150 then" },
  "recent": [ { "turn_id": 3, "speaker": "Buyer", "text": "Too high for me." } ],
  "memory": [
    { "id": "6c1f…", "kind": "constraint", "subject": "user",
      "text": "The user's budget is at most $1,200 including delivery.",
      "source": "conversation c1794…-1-0, turns 3", "saved_ms": 1794690201912 }
  ]
}
```
- `asPrompt(notes)` gives the same notes as prompt lines, for example `- [constraint] user: The user's budget … (id 6c1f…)`. Quote the `id` in the decision's `memory_used` so the UI can link to the note.
- The Live tab's right-hand pane shows exactly the `memory` retrieval picks for the selected or latest turn.
- The Memory tab's "As the decision API receives it" section shows every confirmed note in both formats.

## Decision component (deferred)

Memory is local to the browser. This branch does not POST to the proposed memory endpoint or claim backend acknowledgement. The separate decision component must implement versioned, acknowledged replacement, reject stale revisions, and invalidate pending cues after memory changes. No decision generation is implemented here.

## Updating memory by gesture: wave → speak → wave

Waving toggles continuous memory capture, including while decisions mode is off.

1. Wave three times, in any direction, within 8 s (`THREE_WAVES`, `THREE_WAVE_WINDOW_MS`). Each wave is five fast alternating gyro swings within 3 s, detected by `WaveDetector` in `pipeline/band_pipeline.py` (unchanged, 1.5 s quiet after each). One or two waves do not start or stop capture.
2. Rust receives `set_memory_mode(true)` and broadcasts `{"type":"gesture","name":"wave","memory":true,"time_ms":...}`. The band flashes white for 0.8 s with the mic off, then listens (blue).
3. Every spoken turn until the next wave is `purpose:"context"` / Rust `Purpose::Context`. Pauses do not stop capture. Decision cues are suspended during capture; context never joins conversation history.
4. Each final turn is saved exactly as received, separated by a newline, in one confirmed memory item for this capture. No extraction, model call, editing step or confirmation is required. Existing memory is kept. Saving each finished turn also preserves it across a browser reload.
5. Wave three times again. The mic stops and the speech connection drains before Rust receives `set_memory_mode(false)`. The UI closes the capture indicator. If decisions mode was on, conversation listening resumes; otherwise the mic and LED turn off after the white flash.
6. Triple-tapping also stops and saves any active memory capture before switching decisions mode. A new speech connection at each boundary prevents conversation and memory words from mixing.

Mode, memory and actual mic status are sent to a UI connecting later. Native EventSource reconnect can catch up within the bounded 128-event/1-MiB replay window; gaps outside that window are disclosed. The UI must remain open during recording to save new speech. Memory and conversation history are kept in that browser's localStorage. The proposed orchestrator memory endpoint is still not implemented.

Calibration on the real band remains necessary:
- `VEXO_GESTURE_DEBUG=1` prints tap events and peak turning rate on each gyro axis.
- Adjust `WAVE_AXIS`, `WAVE_DPS`, and tap sensitivity from actual measurements.
- In typed mode, `/wave` runs the same toggle callback (a simulated three-wave gesture) and `/triple` toggles decisions. Typed context is never sent to the copilot as trigger `three_waves` (src/main.rs prints a note instead).

## Verification

From the project root:

```sh
PYO3_PYTHON="$PWD/.venv/bin/python3" cargo test
PYO3_PYTHON="$PWD/.venv/bin/python3" cargo build
.venv/bin/python3 pipeline/test_band_pipeline.py
.venv/bin/python3 pipeline/test_bridge.py
(cd web && npm test && npx tsc --noEmit && npm run build)
```

`test_band_pipeline.py` exercises the actual registered gesture handlers with fake hardware, checks LED/mic ordering and five-blink timing, drains the last words at mode boundaries, and verifies failures stay off. `test_bridge.py` runs the actual embedded Python/Rust process in typed mode, reads SSE, and folds those events through the frontend reducer to check memory, conversation history, clearing, and reload. These checks do not replace physically tapping/waving and observing the LED.

See the root README for setup, explicit retention budgets, persistence errors, and the known microphone gaps during LED signals.
