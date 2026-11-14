// The one orchestrator call (PROPOSED in docs/frontend-plan.md): tell it the mode, goals and
// lunch options. Reached through the /api rewrite in next.config.mjs; throws when the
// orchestrator isn't there. Past decisions and memory are NOT fetched: they are kept in
// this browser (see lib/useBridge.tsx).
import type { Session } from './store';

export async function setSession(session: Session): Promise<void> {
  const r = await fetch('/api/session', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(session) });
  if (!r.ok) throw new Error(`orchestrator answered ${r.status}`);
}
