// Calls from the browser.
// - setSession, syncMemory: the orchestrator (PROPOSED in docs/frontend-plan.md), reached
//   through the /api rewrite in next.config.mjs; they throw when it isn't there.
// - extractMemory: this app's own server route, app/llm/memory/route.ts (OpenAI).
import { MEMORY_KINDS, type MemoryItem, type MemorySource, type Session } from './store';
import { words, type DecisionMemory } from './memory';

const post = (url: string, body: unknown) =>
  fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });

export async function setSession(session: Session): Promise<void> {
  const r = await post('/api/session', session);
  if (!r.ok) throw new Error(`orchestrator answered ${r.status}`);
}

/** Keeps the orchestrator's copy of confirmed memory current, so decisions can use it. */
export async function syncMemory(memories: DecisionMemory[]): Promise<void> {
  const r = await post('/api/memory', { memories });
  if (!r.ok) throw new Error(`orchestrator answered ${r.status}`);
}

/** Notes from text the user gave (typed, or spoken after a wave). With no model, one note
 *  per line as given, so the user's context is never lost; `warning` says why. */
export async function rememberContext(text: string): Promise<{ items: MemoryItem[]; warning?: string }> {
  try {
    return { items: await extractMemory({ source: 'user', text }) };
  } catch (e) {
    const now = Date.now();
    const items: MemoryItem[] = text.split('\n').map((l) => l.trim()).filter(Boolean).map((line) => ({
      id: crypto.randomUUID(), status: 'confirmed', kind: 'context', subject: 'user', text: line,
      keywords: [...new Set(words(line))].slice(0, 8), source: { type: 'user' }, saved_ms: now,
    }));
    return { items, warning: `Saved as given, one note per line. (${(e as Error).message})` };
  }
}

type Extracted = { kind: string; subject: string; text: string; keywords: string[]; turn_ids: number[] };

/** Memory notes from a conversation transcript or from typed context, as new items. */
export async function extractMemory(
  input: { source: 'conversation'; conversation_id: string; transcript: string } | { source: 'user'; text: string },
): Promise<MemoryItem[]> {
  const r = await post('/llm/memory', input);
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error ?? `memory extraction answered ${r.status}`);
  const now = Date.now();
  return (j.memories as Extracted[])
    .filter((m) => typeof m?.text === 'string' && m.text.trim() && MEMORY_KINDS.includes(m.kind as MemoryItem['kind']))
    .map((m) => {
      const source: MemorySource = input.source === 'user'
        ? { type: 'user' }
        : { type: 'conversation', conversation_id: input.conversation_id, turn_ids: m.turn_ids ?? [] };
      return {
        id: crypto.randomUUID(), kind: m.kind as MemoryItem['kind'], subject: m.subject || 'user', text: m.text.trim(),
        keywords: (m.keywords ?? []).map((k) => k.toLowerCase()), source, saved_ms: now,
        // What the user typed is theirs to trust; what was heard waits for confirmation.
        status: input.source === 'user' ? 'confirmed' : 'proposed',
      };
    });
}
