// Event shapes from docs/frontend-plan.md, and the reducer that folds them into UI state.
// No runtime imports here, so `node --test` can load this file directly.

export type Cue = 'Proceed' | 'Pause' | 'Ask' | "Lunch('A')" | "Lunch('B')" | "Lunch('C')" | 'Off';
export type Mode = 'negotiation' | 'lunch';
export type Option = 'A' | 'B' | 'C';

export type TranscriptEvent = {
  type: 'transcript'; turn_id: number; speaker: number | null; // 0-based: display speaker + 1
  text: string; final: boolean; time_ms: number;
  purpose?: 'conversation' | 'context'; // context = the user's spoken update after a wave gesture
};
export type GestureEvent = { type: 'gesture'; name: 'wave'; memory: boolean; time_ms: number };
// Triple tap on the band. On opens a conversation; off closes it. Memory is independent.
export type ModeEvent = { type: 'mode'; decisions: boolean; conversation: number; time_ms: number };
export type CueEvent = { type: 'cue'; turn_id: number; cue: Cue; shown: boolean; time_ms: number };
export type MemoryRef = { id: string; text: string; retrieved_ms: number }; // the memory a decision used
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
// Memory notes, including verbatim wave transcripts. lib/memory.ts retrieves them for a
// turn and formats them for the decision API.
export type MemoryKind = 'preference' | 'constraint' | 'fact' | 'commitment' | 'context';
export const MEMORY_KINDS: MemoryKind[] = ['constraint', 'preference', 'commitment', 'fact', 'context'];
export type MemorySource =
  | { type: 'conversation'; conversation_id: string; turn_ids: number[] } // extracted from what was said
  | { type: 'user' };                                                   // context the user typed in
export type MemoryItem = {
  id: string; status: 'confirmed' | 'proposed';
  kind: MemoryKind;
  subject: string;    // who it is about: a speaker's name, "Speaker 2", or "user"
  text: string;       // e.g. "The user's budget is at most $1,200 including delivery."
  keywords: string[]; // lower-case words retrieval matches on: ["budget", "1200", "delivery"]
  source: MemorySource; saved_ms: number; last_used_ms?: number;
};

// A finished turn as kept with its conversation (interim text is never kept).
export type ConversationTurn = { turn_id: number; speaker: number | null; text: string; time_ms: number };
export type Conversation = { id: string; started_ms: number; mode: Mode; turns: ConversationTurn[]; extracted_ms?: number };
export type MemoryEvent = { type: 'memory'; op: 'proposed' | 'confirmed' | 'updated' | 'deleted'; item: MemoryItem };
export type StatusEvent = { type: 'status'; band: 'connected' | 'reconnecting' | 'off'; mic: 'live' | 'cue-hold' | 'off' };
export type BridgeEvent = TranscriptEvent | CueEvent | DecisionEvent | MemoryEvent | StatusEvent | GestureEvent | ModeEvent;
export const BRIDGE_EVENTS = new Set(['transcript', 'cue', 'decision', 'memory', 'status', 'gesture', 'mode']);

// Spoken context between waves. Each final turn is saved as given, without a model call.
export type PendingContext = { listening: boolean; text: string; interim: string; turn_ids: number[]; time_ms: number };

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
  conversations: Record<string, Conversation>;
  current: string | null; // conversation that new speech joins; null = the next speech starts one
  context: PendingContext | null;
  decisionsOn: boolean | null; // null = the bridge has not said (not connected yet)
  modeTime: number | null; // distinguishes a replay from a session missed while disconnected
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
  | { type: 'forget-decisions' }
  | { type: 'new-conversation' }
  | { type: 'extracted'; id: string; time_ms: number }
  | { type: 'delete-conversation'; id: string };

// What is kept in this browser's localStorage between visits.
export type Saved = Pick<State, 'decisions' | 'memory' | 'session' | 'conversations' | 'current'>;

export const initial: State = {
  turns: {}, decisions: {}, memory: {}, conversations: {}, current: null, context: null, decisionsOn: null, modeTime: null, status: null,
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
      if (a.purpose === 'context') return heardContext(s, a);
      if (s.decisionsOn === false || s.context) return s;
      if (a.final && a.text) return keep(s, a);
      const prev = s.turns[a.turn_id];
      if (prev?.final && !a.final) return s; // interim never overwrites the final text
      const { type: _, ...t } = a;
      return { ...s, turns: { ...s.turns, [a.turn_id]: { ...prev, ...t } } };
    }
    case 'cue': {
      if (s.decisionsOn === false || s.context) return s;
      const turn = { ...turnOf(s, a.turn_id, a.time_ms), cue: a.cue, shown: a.shown };
      const d = turn.decisionId ? s.decisions[turn.decisionId] : undefined;
      return {
        ...s, latest: a.turn_id,
        turns: { ...s.turns, [a.turn_id]: turn },
        decisions: d ? { ...s.decisions, [d.id]: { ...d, shown: a.shown } } : s.decisions, // remember late cues
      };
    }
    case 'decision': {
      if (s.decisionsOn === false || s.context) return s;
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
    case 'mode': {
      if (s.modeTime === a.time_ms && s.decisionsOn === a.decisions) return s; // replayed on reconnect
      // Off: the conversation ends (dropped if nobody spoke). On: a new one opens, the screen clears.
      const conv = s.current ? s.conversations[s.current] : undefined;
      const { [conv?.id ?? '']: empty, ...others } = s.conversations;
      const conversations = conv && conv.turns.length === 0 ? others : s.conversations;
      if (!a.decisions) return { ...s, decisionsOn: false, modeTime: a.time_ms, current: null, conversations, turns: {}, latest: null, pinned: null };
      const c: Conversation = { id: `c${a.time_ms}-m${a.conversation}`, started_ms: a.time_ms, mode: s.session.mode, turns: [] };
      const restored = conversations[c.id] ?? c;
      return { ...s, decisionsOn: true, modeTime: a.time_ms, current: c.id, conversations: { ...conversations, [c.id]: restored }, turns: Object.fromEntries(restored.turns.map((t) => [t.turn_id, { ...t, final: true }])), latest: null, pinned: null };
    }
    case 'gesture':
      if (!a.memory) return { ...s, context: null };
      if (s.context?.time_ms === a.time_ms) return s;
      return { ...s, context: { listening: true, text: s.memory[`wave-${a.time_ms}`]?.text ?? '', interim: '', turn_ids: [], time_ms: a.time_ms } };
    case 'status':
      return { ...s, status: { band: a.band, mic: a.mic } };
    case 'pin':
      return { ...s, pinned: a.turn_id };
    case 'session':
      return { ...s, session: a.session };
    case 'load': { // anything that already arrived live wins over the saved copy
      // Notes saved before memory became sentences (key/value) are dropped, not shown broken.
      const memory = Object.fromEntries(Object.entries(a.saved.memory ?? {}).filter(([, m]) => typeof m?.text === 'string'));
      return {
        ...s, loaded: true,
        decisions: { ...a.saved.decisions, ...s.decisions },
        memory: { ...memory, ...s.memory },
        conversations: { ...a.saved.conversations, ...s.conversations },
        current: s.decisionsOn === false ? null : s.current ?? a.saved.current ?? null,
        session: a.saved.session ?? s.session,
      };
    }
    case 'forget-decisions':
      return { ...s, decisions: {}, turns: Object.fromEntries(Object.entries(s.turns).map(([id, { decisionId: _, ...t }]) => [id, t])) };
    case 'new-conversation':
      return { ...s, current: null, turns: {}, latest: null, pinned: null };
    case 'extracted': {
      const c = s.conversations[a.id];
      return c ? { ...s, conversations: { ...s.conversations, [a.id]: { ...c, extracted_ms: a.time_ms } } } : s;
    }
    case 'delete-conversation': {
      const { [a.id]: _, ...rest } = s.conversations;
      return { ...s, conversations: rest, current: s.current === a.id ? null : s.current };
    }
    default:
      return s;
  }
}

// Spoken context never joins the conversation. Preserve each final turn exactly, separated
// by a newline. Save as it arrives so a reload cannot discard the captured memory.
function heardContext(s: State, a: TranscriptEvent): State {
  const c = s.context;
  if (!c || c.turn_ids.includes(a.turn_id)) return s;
  if (!a.final) return { ...s, context: { ...c, interim: a.text } };
  if (!a.text) return s;
  const text = c.text ? `${c.text}\n${a.text}` : a.text;
  const item: MemoryItem = {
    id: `wave-${c.time_ms}`, status: 'confirmed', kind: 'context', subject: 'user', text,
    keywords: [], source: { type: 'user' }, saved_ms: a.time_ms,
  };
  return { ...s, context: { ...c, interim: '', text, turn_ids: [...c.turn_ids, a.turn_id] }, memory: { ...s.memory, [item.id]: item } };
}

// A final turn: show it, and keep it in the current conversation. The bridge numbers turns
// from 1 each time it starts, so a turn number that does not go up means a new run of the
// bridge: that starts a new conversation (and clears the old run's turns from the screen).
function keep(s: State, a: TranscriptEvent): State {
  const conv = s.current ? s.conversations[s.current] : undefined;
  const last = conv?.turns.at(-1);
  const restarted = last !== undefined && a.turn_id <= last.turn_id;
  const c: Conversation = conv && !restarted
    ? conv
    : { id: `c${a.time_ms}-${a.turn_id}-${Object.keys(s.conversations).length}`, started_ms: a.time_ms, mode: s.session.mode, turns: [] };
  const { type: _, ...t } = a;
  const turns = restarted ? {} : s.turns;
  return {
    ...s, current: c.id,
    turns: { ...turns, [a.turn_id]: { ...turns[a.turn_id], ...t } },
    conversations: {
      ...s.conversations,
      [c.id]: { ...c, turns: [...c.turns, { turn_id: a.turn_id, speaker: a.speaker, text: a.text, time_ms: a.time_ms }] },
    },
    ...(restarted ? { latest: null, pinned: null } : {}),
  };
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}
