// Orchestrator HTTP API (all PROPOSED in docs/frontend-plan.md), reached through the
// /api rewrite in next.config.mjs. Every call throws when the orchestrator isn't there;
// callers fall back to what the event stream has already delivered.
import type { DecisionEvent, MemoryItem, Session, Turn } from './store';

async function call<T>(path: string, method = 'GET', body?: unknown): Promise<T> {
  const r = await fetch('/api' + path, {
    method,
    headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error(`orchestrator answered ${r.status}`);
  const text = await r.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

const list = <T,>(v: unknown): T[] => {
  if (!Array.isArray(v)) throw new Error('orchestrator sent an unexpected shape');
  return v;
};
const id = encodeURIComponent;

export const api = {
  setSession: (s: Session) => call<void>('/session', 'POST', s),
  transcript: () => call<unknown>('/session/transcript').then(list<Turn>),
  decisions: (q: { mode?: string; cue?: string; session?: string }) =>
    call<unknown>('/decisions?' + new URLSearchParams(Object.entries(q).filter(([, v]) => v) as [string, string][])).then(list<DecisionEvent>),
  decision: (decisionId: string) => call<DecisionEvent>(`/decisions/${id(decisionId)}`),
  memory: () => call<unknown>('/memory').then(list<MemoryItem>),
  confirmMemory: (memoryId: string) => call<void>(`/memory/${id(memoryId)}/confirm`, 'POST'),
  patchMemory: (memoryId: string, patch: Partial<Pick<MemoryItem, 'key' | 'value'>>) => call<void>(`/memory/${id(memoryId)}`, 'PATCH', patch),
  deleteMemory: (memoryId: string) => call<void>(`/memory/${id(memoryId)}`, 'DELETE'),
};
