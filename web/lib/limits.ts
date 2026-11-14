// Explicit retention budgets. Mandatory memory is rejected with a notice, never pruned.
import type { State } from './store.ts';

export const MAX_TEXT_BYTES = 8192;
export const MAX_MEMORY_BYTES = 256 * 1024;
export const MAX_NOTE_BYTES = 32 * 1024;
export const MAX_HISTORY_BYTES = 1024 * 1024;
export const MAX_CONVERSATIONS = 50;
export const MAX_TURNS = 200;
export const MAX_DECISIONS = 200;
const encoder = new TextEncoder();
export const byteSize = (value: unknown) => encoder.encode(JSON.stringify(value)).byteLength;

export function boundState(state: State): State {
  let s = state;
  let trimmed = false;
  const turns = Object.values(s.turns).sort((a, b) => a.turn_id - b.turn_id);
  if (turns.length > MAX_TURNS) s = { ...s, turns: Object.fromEntries(turns.slice(-MAX_TURNS).map((t) => [t.turn_id, t])) };
  const decisions = Object.values(s.decisions).sort((a, b) => b.time_ms - a.time_ms);
  if (decisions.length > MAX_DECISIONS) {
    s = { ...s, decisions: Object.fromEntries(decisions.slice(0, MAX_DECISIONS).map((d) => [d.id, d])) };
    trimmed = true;
  }
  let conversations = Object.values(s.conversations).sort((a, b) => b.started_ms - a.started_ms);
  conversations = conversations.map((c) => {
    if (c.turns.length <= MAX_TURNS) return c;
    trimmed = true;
    return { ...c, turns: c.turns.slice(-MAX_TURNS) };
  });
  if (conversations.length > MAX_CONVERSATIONS) {
    const current = conversations.find((c) => c.id === s.current);
    conversations = conversations.slice(0, MAX_CONVERSATIONS);
    if (current && !conversations.includes(current)) conversations[MAX_CONVERSATIONS - 1] = current;
    trimmed = true;
  }
  // Bound both decisions (including their retained context) and conversation text by bytes.
  const history = { conversations: Object.fromEntries(conversations.map((c) => [c.id, c])), decisions: s.decisions };
  while (byteSize(history) > MAX_HISTORY_BYTES) {
    const oldest = conversations.filter((c) => c.id !== s.current).at(-1);
    if (oldest) {
      conversations = conversations.filter((c) => c.id !== oldest.id);
      delete history.conversations[oldest.id];
    } else {
      const decision = Object.values(history.decisions).sort((a, b) => a.time_ms - b.time_ms)[0];
      if (decision) {
        history.decisions = { ...history.decisions };
        delete history.decisions[decision.id];
      } else {
        const current = conversations.find((c) => c.id === s.current);
        if (!current?.turns.length) break;
        const shortened = { ...current, turns: current.turns.slice(1) };
        conversations = [shortened];
        history.conversations[current.id] = shortened;
      }
    }
    trimmed = true;
  }
  if (trimmed) {
    s = { ...s, conversations: history.conversations, decisions: history.decisions,
      notice: 'History retention limit reached. Older turns or conversations were removed; export anything you need to keep.' };
  }
  return s;
}
