// Memory retrieval and the format the decision API is fed. No runtime imports from the app,
// so `node --test` can load it.
//
// Retrieval is keyword overlap: a note scores 2 for each of its keywords the turn mentions and
// 1 for each other word of its text, plus 1 when it is about the person speaking. Constraints
// and the user's own context are always included: a decision must never forget a goal.
// ponytail: keyword overlap misses synonyms ("cost" vs "price"); add embeddings when notes
// outnumber a few hundred or misses show up in the demo.
import type { Conversation, MemoryItem } from './store.ts';

const STOP = new Set(
  'a an and are as at be but by can do for from has have i if in is it its me my no not of on or our so that the their them they this to was we were what when will with you your'.split(' '),
);

/** Lower-case words worth matching on. "$1,200" becomes "1200". */
export function words(text: string): string[] {
  const out = text.toLowerCase().replace(/(\d),(?=\d{3})/g, '$1').match(/[a-z0-9]+/g) ?? [];
  return out.filter((w) => !STOP.has(w) && (w.length > 1 || /\d/.test(w)));
}

const always = (m: MemoryItem) => m.kind === 'constraint' || m.source.type === 'user';

/** The confirmed notes relevant to `query` (usually the latest turn), best first. */
export function retrieve(items: MemoryItem[], query: string, opts: { subject?: string; k?: number } = {}): MemoryItem[] {
  const q = new Set(words(query));
  const scored = items
    .filter((m) => m.status === 'confirmed')
    .map((m) => {
      const keys = new Set(m.keywords.map((k) => k.toLowerCase()));
      let score = 0;
      for (const w of keys) if (q.has(w)) score += 2;
      for (const w of new Set(words(m.text))) if (!keys.has(w) && q.has(w)) score += 1;
      if (opts.subject && m.subject === opts.subject) score += 1;
      return { m, score };
    })
    .filter(({ m, score }) => score > 0 || always(m))
    .sort((a, b) => b.score - a.score || b.m.saved_ms - a.m.saved_ms);
  const optional = new Set(scored.filter(({ m }) => !always(m)).slice(0, Math.max(0, opts.k ?? 8)).map(({ m }) => m.id));
  return scored.filter(({ m }) => always(m) || optional.has(m.id)).map(({ m }) => m);
}

/** One note as the decision API receives it. */
export type DecisionMemory = {
  id: string; kind: MemoryItem['kind']; subject: string; text: string;
  source: string;    // "user" or "conversation <id>, turns 3,4"
  saved_ms: number;
};

export function forDecision(items: MemoryItem[]): DecisionMemory[] {
  return items.map((m) => ({
    id: m.id, kind: m.kind, subject: m.subject, text: m.text, saved_ms: m.saved_ms,
    source: m.source.type === 'user' ? 'user' : `conversation ${m.source.conversation_id}, turns ${m.source.turn_ids.join(',')}`,
  }));
}

/** The same notes as lines to paste into a model prompt: "- [constraint] user: …". */
export function asPrompt(items: MemoryItem[]): string {
  return items.map((m) => `- [${m.kind}] ${m.subject}: ${m.text} (id ${m.id})`).join('\n');
}

/** What the decision API is sent for one turn: the turn, a few turns before it, and memory. */
export type DecisionRequest = {
  turn: { turn_id: number; speaker: string; text: string };
  recent: { turn_id: number; speaker: string; text: string }[];
  memory: DecisionMemory[];
};

export function decisionRequest(conv: Conversation, turn_id: number, items: MemoryItem[], name: (speaker: number | null) => string): DecisionRequest | null {
  const i = conv.turns.findIndex((t) => t.turn_id === turn_id);
  if (i < 0) return null;
  const view = (t: Conversation['turns'][number]) => ({ turn_id: t.turn_id, speaker: name(t.speaker), text: t.text });
  const turn = view(conv.turns[i]);
  return {
    turn,
    recent: conv.turns.slice(Math.max(0, i - 6), i).map(view),
    memory: forDecision(retrieve(items, turn.text, { subject: turn.speaker })),
  };
}

/** A conversation as plain lines for the memory extractor: "t3 Seller: I can do $1,150." */
export function transcriptLines(conv: Conversation, name: (speaker: number | null) => string): string {
  return conv.turns.map((t) => `t${t.turn_id} ${name(t.speaker)}: ${t.text}`).join('\n');
}
