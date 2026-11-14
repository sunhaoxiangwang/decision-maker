# Companion UI: implementation outline (Next.js)

The companion app shows a live conversation between several people, the decision agent's
verdict for each turn, and the memory that verdict relied on. Two more tabs cover past
decisions and the user's stored memory.

**Status of each integration:** **Built** means it exists in this repo (`src/bridge.rs`).
**Proposed** means the orchestrator owner still has to implement it to the shape given here.
The UI should code against these shapes and fall back gracefully until they exist.

---

## 1. Screens

### Tab 1: Live

```
┌──────────────────────────────────────────────┬──────────────────────────────┐
│ Mode: [Negotiation ▾]  Goals ✓  ● Mic live   │  DECISION · turn 4           │
│ "Recording — audio goes to Deepgram"          │  ██ ASK (amber)              │
├──────────────────────────────────────────────┤  Return policy not stated.   │
│ Speaker 1   $1,399 delivered.          t1  ● │  Missing: return_nights      │
│ Speaker 2   Too high for me.           t2    │  ───────────────────────────  │
│ Speaker 1   $1,150, 30-day return.     t3  ● │  FACTS USED                  │
│ Speaker 1   OK, $1,150 then…  (typing) t4    │  price_total = 1150 (t3)     │
│                                              │  ───────────────────────────  │
│                                              │  MEMORY USED                 │
│                                              │  • budget ≤ $1,200 incl.     │
│                                              │    delivery · confirmed · 2h │
│                                              │  • ≥100-night return · goal  │
└──────────────────────────────────────────────┴──────────────────────────────┘
```

- **Conversation pane:**
  - One bubble per turn.
  - Each speaker gets a stable colour, and its name can be edited ("Speaker 1" can become "Seller").
  - Interim text is shown greyed and in italics, then replaced in place when the final text for that `turn_id` arrives.
  - A dot on a turn shows the colour of the cue it produced. Click the turn to open its decision in the right pane.
- **Decision pane:**
  - Shows the latest decision, or the one you clicked: the cue as a colour plus a label, the reason, and any missing facts.
  - Below that come the facts used and the memory used. Each item shows its source and its age or retrieval time.
  - Superseded cues (`shown:false`) appear struck through with the note "late — not sent to band".
- **Header:**
  - Mode switch: Negotiation or Lunch.
  - Goals form, which must be confirmed before listening counts.
  - Mic state: live, paused for a cue, or off.
  - A disclosure banner whenever the mic is live. This is required by the kit's safety rules.
- **Lunch mode** adds an options editor for A, B and C. The fixed colours are A = green, B = purple, C = white. When no option is chosen, the pane says why and lights no option.

### Tab 2: Decisions (history)
- A table with time, mode, cue, a one-line reason, the turn, and latency (speech → decision → LED, in ms).
- Filters: mode, cue, and session.
- A detail drawer shows:
  - the turns that led up to the decision;
  - the facts, separated into confirmed and missing;
  - the memory items used, linked to the Memory tab;
  - the raw agent output, collapsed by default.

### Tab 3: Memory
- **Confirmed** preferences and facts: key, value, source (a session or turn, Maps/Places, or the user), and when it was saved and last used. Each can be edited or deleted.
- **Proposed** memory from post-session summaries (OpenAI), which waits for the user to confirm or reject it. These must never be treated as confirmed: confidence alone doesn't confirm a fact.
- Each item shows how many decisions used it, and clicking that count filters Tab 2.

---

## 2. Integrations with the backend

| # | UI need | Backend function | Transport | Status |
|---|---|---|---|---|
| 1 | Live transcript (interim and final, speaker, turn) | `emit` → bridge, streamed by the `Hub` in `src/bridge.rs` | Server-Sent Events `GET http://127.0.0.1:8787/events`, event `transcript` | **Built** |
| 2 | Cue sent to the band | `Bridge::show(turn_id, cue)` | Server-Sent Events, event `cue` | **Built** |
| 3 | Decision with reason, facts and memory used | Orchestrator calls `Bridge::publish(json)` after deciding | Server-Sent Events, event `decision` | **Proposed** (`publish` is built) |
| 4 | Memory changes during a session | Orchestrator calls `Bridge::publish` | Server-Sent Events, event `memory` | **Proposed** |
| 5 | Mic and band status | Python pipeline → bridge publish | Server-Sent Events, event `status` | **Proposed** |
| 6 | Set mode, goals and lunch options | Orchestrator `POST /session` | HTTP JSON | **Proposed** |
| 7 | Past decisions | Orchestrator `GET /decisions?mode=&cue=&session=`, `GET /decisions/:id` | HTTP JSON | **Proposed** |
| 8 | Memory for decisions | UI → orchestrator `POST /api/memory` with every confirmed note whenever memory changes. The memory itself lives in the browser (see `docs/memory.md`) | HTTP JSON | UI side **built**, orchestrator **proposed** |
| 9 | Catch up after a reload | Orchestrator `GET /session/transcript` (the Server-Sent Events stream has no backlog) | HTTP JSON | **Proposed** |

### Connection rules (already enforced by the bridge)
- It listens on `127.0.0.1` only.
- A browser may read it only from `BRIDGE_UI_ORIGIN`, which defaults to `http://localhost:3000`, and only through a localhost host name. Any other origin gets a 403.
- Read the stream directly with `new EventSource('http://127.0.0.1:8787/events')`. Don't proxy it through a Next.js rewrite, because rewrites may buffer the stream.
- Proxy the orchestrator's HTTP API with a `next.config.js` rewrite (`/api/:path*` → `http://127.0.0.1:<orchestrator port>/:path*`). That avoids CORS for HTTP requests. The orchestrator should apply the same localhost-only binding and Host check.
- No API keys (Deepgram, OpenAI, Gemini, Jev) ever go to the browser.

### Event shapes (Server-Sent Events `data:` lines, each with a `type` field)

```ts
// Built
type TranscriptEvent = {
  type: 'transcript'; turn_id: number; speaker: number | null; // Deepgram numbers from 0: display speaker + 1
  text: string; final: boolean; time_ms: number;
};
type CueEvent = {
  type: 'cue'; turn_id: number;
  cue: 'Proceed' | 'Pause' | 'Ask' | "Lunch('A')" | "Lunch('B')" | "Lunch('C')" | 'Off';
  shown: boolean;               // false = stale turn, the band was not touched
  time_ms: number;
};

// Proposed: the orchestrator publishes these through Bridge::publish
type DecisionEvent = {
  type: 'decision'; id: string; turn_id: number; mode: 'negotiation' | 'lunch';
  cue: CueEvent['cue']; reason: string;
  facts: { key: string; value: string; turn_id: number; confirmed: boolean }[];
  missing: string[];            // amber "Ask" = required facts not yet stated
  conflicts: string[];          // red "Pause" = a known fact breaks a goal
  memory_used: MemoryRef[];
  choice?: { option: 'A' | 'B' | 'C' | null; eligible: ('A' | 'B' | 'C')[] };   // lunch
  latency_ms: { transcript_to_decision: number; decision_to_led?: number };
  time_ms: number;
};
type MemoryRef = { id: string; text: string; retrieved_ms: number };  // see docs/memory.md
type MemoryEvent = { type: 'memory'; op: 'proposed' | 'confirmed' | 'updated' | 'deleted'; item: MemoryItem };
// Built: triple tap on the band switches decisions mode; replayed to every UI that connects.
type ModeEvent = { type: 'mode'; decisions: boolean; conversation: number; time_ms: number };
type GestureEvent = { type: 'gesture'; name: 'wave'; time_ms: number };
type StatusEvent = { type: 'status'; band: 'connected' | 'reconnecting' | 'off'; mic: 'live' | 'cue-hold' | 'off' };

// Proposed: HTTP JSON
// MemoryItem: a sentence about one subject. Built: see docs/memory.md and web/lib/store.ts.
type Session = {
  mode: 'negotiation' | 'lunch';
  goals: { key: string; value: string; confirmed: boolean }[];  // e.g. budget_total ≤ 1200
  lunch_options?: { A: string; B: string; C: string };
};
```

How the pieces join up:
- `turn_id` links each transcript turn to its cue and its decision.
- `MemoryRef.id` links a decision to an item in the Memory tab.

---

## 3. Frontend structure (Next.js App Router)

```
app/
  layout.tsx            tabs: Live | Decisions | Memory
  page.tsx              Live tab
  decisions/page.tsx    history table and detail drawer
  memory/page.tsx       confirmed and proposed lists
lib/
  useBridge.ts          one EventSource for the whole app, in a context provider
  store.ts              reducer: turns, decisions and memory, keyed by id
  api.ts                fetch wrappers for /api/session, /decisions and /memory
components/
  Conversation.tsx  DecisionCard.tsx  MemoryList.tsx  CueBadge.tsx  GoalsForm.tsx
```

### The reducer (`lib/store.ts`)
- **`transcript`:**
  - Upsert `turns[turn_id]`.
  - A final event replaces interim text.
  - The speaker comes from the latest event for that turn.
- **`cue`:** set `turns[turn_id].cue` and `shown`.
- **`decision`:**
  - Store it in `decisions[id]` and set `turns[turn_id].decisionId`.
  - Make it the selected decision if the user hasn't pinned one.
- **`memory`:** upsert or remove the item in `memory[id]`.
- **`status`:** replace the header state.

### Reconnecting
- `EventSource` reconnects by itself.
- On `onopen` after an error, call `GET /session/transcript`, `GET /decisions?session=current` and `GET /memory` to fill the gap.
- Speaker names are kept in `localStorage` per session. That's a per-viewer convenience, not shared state.

### Safety in the UI
- Render all transcript and memory text as text (React does this by default), never with `dangerouslySetInnerHTML`. Speech is untrusted input.
- Show the recording disclosure banner whenever `status.mic === 'live'`.

---

## 4. Build order

| Step | Work | Needs |
|---|---|---|
| 1 | `useBridge`, the reducer and the Conversation pane on events #1 and #2. Test with `cargo run -- --typed --cue`. | Bridge only (works today) |
| 2 | DecisionCard with mock `decision` events (a fixture file of the 4 negotiation cases and the lunch cases). | Nothing |
| 3 | GoalsForm and mode switch → `POST /session`. | Orchestrator #6 |
| 4 | Decisions tab → `GET /decisions`. | Orchestrator #7 |
| 5 | Memory tab with confirm, reject and edit → `/memory`. | Orchestrator #8 |
| 6 | Status header, reconnect catch-up, latency column. | #5 and #9 |

Steps 1–2 can be shown in the demo even if the orchestrator's HTTP API isn't ready. Label anything driven by mock data as such.

### Demo check (expected cues, from the build plan)

| Spoken | Expected cue | What the UI must show |
|---|---|---|
| "$1,399" | Pause, red | Conflict: over the $1,200 budget, with the budget memory cited |
| "$1,150, 30-day return" | Pause, red | Conflict: return policy under 100 nights |
| "$1,150" (no return policy stated) | Ask, amber | Missing: return policy |
| "$1,150, 100-night return, delivery included" | Proceed, green | All goals satisfied, with the facts listed |
| Lunch with A, B and C | Option colour, or no cue | Eligible options, the reason, and the sources with retrieval times |
