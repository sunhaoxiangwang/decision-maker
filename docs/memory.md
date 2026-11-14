# Memory: how it's built, stored and fed to decisions

Memory is written as notes in plain sentences, the way an LLM would write them. Each note is
one self-contained sentence about one subject, with where it came from. It is kept in the
companion UI (the browser's localStorage) and sent to the orchestrator whenever it changes.

## Where notes come from

| Source | How | Status |
|---|---|---|
| **Context the user types** (Memory tab, "Remember") | Sent to `POST /llm/memory` with `{source:"user", text}`. OpenAI splits it into notes. With no OpenAI key, each line is saved as it was typed. | `confirmed`: the user said it |
| **Past conversations** (Memory tab → Past conversations → "Remember this conversation") | The saved transcript is sent as lines (`t3 Seller: I can do $1,150`) to `POST /llm/memory` with `{source:"conversation", conversation_id, transcript}` | `proposed`: never used until the user confirms it |

Conversations are saved automatically:
- Every final transcript turn, from the bridge's event stream, joins the current conversation.
- A new conversation starts when you press **New conversation** on the Live tab.
- A new conversation also starts when the bridge restarts. The turn numbers starting again from 1 show that.

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
