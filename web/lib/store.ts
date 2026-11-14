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

// A decision as kept in this browser: the event, plus what is needed to read it later,
// when the turns it came from are gone.
export type Decision = DecisionEvent & { context?: Pick<Turn, 'turn_id' | 'speaker' | 'text'>[]; shown?: boolean };

export type State = {
  turns: Record<number, Turn>;
  decisions: Record<string, Decision>;
  memory: Record<string, MemoryItem>;
  status: { band: StatusEvent['band']; mic: StatusEvent['mic'] } | null; // null = never reported
  session: Session;
  latest: number | null; // turn of the newest cue or decision
  pinned: number | null; // turn the user clicked; wins over `latest`
  loaded: boolean;       // saved decisions and memory have been read from this browser
};

export type Action =
  | BridgeEvent
  | { type: 'pin'; turn_id: number | null }
  | { type: 'session'; session: Session }
  | { type: 'load'; saved: Partial<Saved> }
  | { type: 'forget-decisions' };

// What is kept in this browser's localStorage between visits.
export type Saved = Pick<State, 'decisions' | 'memory' | 'session'>;

export const initial: State = {
  turns: {}, decisions: {}, memory: {}, status: null,
  session: { mode: 'negotiation', goals: [] },
  latest: null, pinned: null, loaded: false,
};

export const selectedTurn = (s: State) => s.pinned ?? s.latest;

// Turns worth drawing, in order. While nobody speaks, Deepgram keeps sending interim
// guesses with empty text (the kit passes them on), which would otherwise show as a
// nameless empty turn at the bottom.
export const visibleTurns = (s: State) =>
  Object.values(s.turns).filter((t) => t.text || t.cue || t.decisionId).sort((a, b) => a.turn_id - b.turn_id);

// A cue or decision can arrive for a turn we never saw spoken (after a reload).
const turnOf = (s: State, turn_id: number, time_ms: number): Turn =>
  s.turns[turn_id] ?? { turn_id, speaker: null, text: '', final: true, time_ms };

export function reducer(s: State, a: Action): State {
  switch (a.type) {
    case 'transcript': {
      const prev = s.turns[a.turn_id];
      if (prev?.final && !a.final) return s; // interim never overwrites the final text
      const { type: _, ...t } = a;
      return { ...s, turns: { ...s.turns, [a.turn_id]: { ...prev, ...t } } };
    }
    case 'cue': {
      const turn = { ...turnOf(s, a.turn_id, a.time_ms), cue: a.cue, shown: a.shown };
      const d = turn.decisionId ? s.decisions[turn.decisionId] : undefined;
      return {
        ...s, latest: a.turn_id,
        turns: { ...s.turns, [a.turn_id]: turn },
        decisions: d ? { ...s.decisions, [d.id]: { ...d, shown: a.shown } } : s.decisions, // remember late cues
      };
    }
    case 'decision': {
      const context = visibleTurns(s).filter((t) => t.text && t.turn_id <= a.turn_id).slice(-4)
        .map(({ turn_id, speaker, text }) => ({ turn_id, speaker, text }));
      return {
        ...s, latest: a.turn_id,
        decisions: { ...s.decisions, [a.id]: { ...a, context } },
        turns: { ...s.turns, [a.turn_id]: { ...turnOf(s, a.turn_id, a.time_ms), decisionId: a.id } },
      };
    }
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
    case 'load': // anything that already arrived live wins over the saved copy
      return {
        ...s, loaded: true,
        decisions: { ...a.saved.decisions, ...s.decisions },
        memory: { ...a.saved.memory, ...s.memory },
        session: a.saved.session ?? s.session,
      };
    case 'forget-decisions':
      return { ...s, decisions: {}, turns: Object.fromEntries(Object.entries(s.turns).map(([id, { decisionId: _, ...t }]) => [id, t])) };
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
