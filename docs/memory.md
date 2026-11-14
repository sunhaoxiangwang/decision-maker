# Memory: how it's built, stored and fed to decisions

Memory is written as notes in plain sentences, the way an LLM would write them. Each note is
one self-contained sentence about one subject, with where it came from. It is kept in the
companion UI (the browser's localStorage) and sent to the orchestrator whenever it changes.

## Where notes come from

| Source | How | Status |
|---|---|---|
| **Context the user types** (Memory tab, "Remember") | Sent to `POST /llm/memory` with `{source:"user", text}`. OpenAI splits it into notes. With no OpenAI key, each line is saved as it was typed. | `confirmed`: the user said it |
| **Past conversations** (Memory tab → Past conversations → "Remember this conversation") | The saved transcript is sent as lines (`t3 Seller: I can do $1,150`) to `POST /llm/memory` with `{source:"conversation", conversation_id, transcript}` | `proposed`: never used until the user confirms it |

Conversations are saved only while **decisions mode** is on:
- **Triple-tap** the band (3 taps within 1.2 s; a `double-tap` counts as 2).
  - The band blinks green 5 times. Then a fresh Deepgram connection opens, so speaker numbers start again, and the mic turns on.
  - Rust gets the handoff (`set_mode(true)`) and every UI receives `{"type":"mode","decisions":true,"conversation":N}`. The UI opens a new conversation and clears the screen.
  - Every finished turn carries `conversation: N`.
- **Triple-tap again**: the band blinks red 5 times and the mic stops.
  - Words already spoken are still sent.
  - After that, nothing is heard, sent or saved, and `Bridge::show` sends no cues to the band.
  - The UI closes the conversation, and drops it if nobody spoke.
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
  - Deepgram speaker numbers are only stable within one run of the bridge.
  - Name the speakers before pressing "Remember", so notes are about "Seller" rather than "Speaker 2".

## Retrieval (`web/lib/memory.ts`)

`retrieve(items, turnText, { subject })` returns the most relevant notes first (8 at most). Only confirmed notes are considered.
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

## Getting memory to the orchestrator (proposed)

- Whenever memory changes, the UI sends `POST /api/memory { memories: DecisionMemory[] }`. The `/api` rewrite in `next.config.mjs` forwards it to the orchestrator.
- The orchestrator keeps that list and runs the same retrieval for each final turn it gets from `Bridge::next_transcript`. The keyword fields are there so this takes a few lines of Rust.
- Until the orchestrator exists, the request fails quietly.

## Updating context by gesture: wave → speak → confirm

Waving only works while decisions mode is on.

1. **Wave** the band left to right three times. In practice that's five fast swings of alternating direction within 3 s on one gyro axis, detected in Python (`WaveDetector` in `pipeline/band_pipeline.py`).
2. The band **flashes white** for 0.8 s. The mic is off while the light shows, then listening resumes. The UI shows "say the updated context now".
3. **Speak.** The next finished turn is tagged `"purpose":"context"` on the transcript event, and `Transcript.purpose == Purpose::Context` in Rust.
   - The orchestrator must not decide on a context turn.
   - The UI keeps it out of the conversation.
   - If a pause splits the update, the next part is *not* captured: every turn after the first is conversation again. Edit it into the text before confirming.
4. **Confirm** on the Live tab, where the text can be edited first. The text goes through the same note-writing as typed context.
5. The new notes **replace** your own context, meaning the confirmed notes whose source is `user`. Notes from conversations are kept.
6. Memory is sent to the orchestrator automatically. Retrieval always includes your own context notes, so every decision from the next turn on uses the new context.

Calibrating on the band:
- Start with `VEXO_GESTURE_DEBUG=1` set in the environment and wave. Once a second it prints the fastest turning rate on each axis.
- Set `WAVE_AXIS` to the axis that moves most when you wave, and `WAVE_DPS` to about half its peak.
- In typed mode, a line `/wave` acts out the gesture.
