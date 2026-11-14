// Event shapes from docs/frontend-plan.md, and the reducer that folds them into UI state.
// No runtime imports here, so `node --test` can load this file directly.

export type Cue = 'Proceed' | 'Pause' | 'Ask' | "Lunch('A')" | "Lunch('B')" | "Lunch('C')" | 'Off';
export type Mode = 'negotiation' | 'lunch';
export type Option = 'A' | 'B' | 'C';

export type TranscriptEvent = {
  type: 'transcript'; turn_id: number; speaker: number | null; // 0-based: display speaker + 1
  text: string; final: boolean; time_ms: number;
};
export type CueEvent = { type: 'cue'; turn_id: number; cue: Cue; shown: boolean; time_ms: number };
export type MemoryRef = { id: string; key: string; value: string; source: string; retrieved_ms: number };
export type DecisionEvent = {
  type: 'decision'; id: string; turn_id: number; mode: Mode; cue: Cue; reason: string;
  facts: { key: string; value: string; turn_id: number; confirmed: boolean }[];
  missing: string[];
  conflicts: string[];
  memory_used: MemoryRef[];
  choice?: { option: Option | null; eligible: Option[] };
  latency_ms: { transcript_to_decision: number; decision_to_led?: number };
  time_ms: number;
};
export type MemoryItem = {
  id: string; key: string; value: string; status: 'confirmed' | 'proposed';
  source: string; saved_ms: number; last_used_ms?: number; used_by: number;
};
export type MemoryEvent = { type: 'memory'; op: 'proposed' | 'confirmed' | 'updated' | 'deleted'; item: MemoryItem };
export type StatusEvent = { type: 'status'; band: 'connected' | 'reconnecting' | 'off'; mic: 'live' | 'cue-hold' | 'off' };
export type BridgeEvent = TranscriptEvent | CueEvent | DecisionEvent | MemoryEvent | StatusEvent;
export const BRIDGE_EVENTS = new Set(['transcript', 'cue', 'decision', 'memory', 'status']);

export type Session = {
  mode: Mode;
  goals: { key: string; value: string; confirmed: boolean }[];
  lunch_options?: { A: string; B: string; C: string };
};

export type Turn = {
  turn_id: number; speaker: number | null; text: string; final: boolean; time_ms: number;
  cue?: Cue; shown?: boolean; decisionId?: string;
};

export type State = {
  turns: Record<number, Turn>;
  decisions: Record<string, DecisionEvent>;
  memory: Record<string, MemoryItem>;
  status: { band: StatusEvent['band']; mic: StatusEvent['mic'] } | null; // null = never reported
  session: Session;
  latest: number | null; // turn of the newest cue or decision
  pinned: number | null; // turn the user clicked; wins over `latest`
  mock: boolean;         // fixture data is on screen
};

export type Action =
  | BridgeEvent
  | { type: 'pin'; turn_id: number | null }
  | { type: 'session'; session: Session }
  | { type: 'hydrate'; turns?: Turn[]; decisions?: DecisionEvent[]; memory?: MemoryItem[] }
  | { type: 'reset'; mock: boolean };

export const initial: State = {
  turns: {}, decisions: {}, memory: {}, status: null,
  session: { mode: 'negotiation', goals: [] },
  latest: null, pinned: null, mock: false,
};

export const selectedTurn = (s: State) => s.pinned ?? s.latest;

// A cue or decision can arrive for a turn we never saw spoken (after a reload).
const turnOf = (s: State, turn_id: number, time_ms: number): Turn =>
  s.turns[turn_id] ?? { turn_id, speaker: null, text: '', final: true, time_ms };

const byKey = <T, K extends string | number>(items: T[] | undefined, key: (t: T) => K) =>
  Object.fromEntries((items ?? []).map((t) => [key(t), t])) as Record<K, T>;

export function reducer(s: State, a: Action): State {
  switch (a.type) {
    case 'transcript': {
      const prev = s.turns[a.turn_id];
      if (prev?.final && !a.final) return s; // interim never overwrites the final text
      const { type: _, ...t } = a;
      return { ...s, turns: { ...s.turns, [a.turn_id]: { ...prev, ...t } } };
    }
    case 'cue':
      return {
        ...s, latest: a.turn_id,
        turns: { ...s.turns, [a.turn_id]: { ...turnOf(s, a.turn_id, a.time_ms), cue: a.cue, shown: a.shown } },
      };
    case 'decision':
      return {
        ...s, latest: a.turn_id,
        decisions: { ...s.decisions, [a.id]: a },
        turns: { ...s.turns, [a.turn_id]: { ...turnOf(s, a.turn_id, a.time_ms), decisionId: a.id } },
      };
    case 'memory': {
      const { [a.item.id]: _, ...rest } = s.memory;
      return { ...s, memory: a.op === 'deleted' ? rest : { ...rest, [a.item.id]: a.item } };
    }
    case 'status':
      return { ...s, status: { band: a.band, mic: a.mic } };
    case 'pin':
      return { ...s, pinned: a.turn_id };
    case 'session':
      return { ...s, session: a.session };
    case 'hydrate': { // fills gaps after a reconnect; anything already seen live wins
      const turns = { ...byKey(a.turns, (t) => t.turn_id), ...s.turns };
      const decisions = { ...byKey(a.decisions, (d) => d.id), ...s.decisions };
      for (const d of Object.values(decisions)) {
        if (turns[d.turn_id] && !turns[d.turn_id].decisionId) turns[d.turn_id] = { ...turns[d.turn_id], decisionId: d.id };
      }
      return { ...s, turns, decisions, memory: { ...byKey(a.memory, (m) => m.id), ...s.memory } };
    }
    case 'reset':
      return { ...initial, mock: a.mock };
    default:
      return s;
  }
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}
