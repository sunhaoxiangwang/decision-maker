'use client';
// One EventSource for the whole app. Read directly from the bridge (never through a
// Next.js rewrite). The bridge only answers http://localhost:3000, so open the UI there.
import { createContext, useContext, useEffect, useReducer, useState, type Dispatch, type ReactNode } from 'react';
import { api } from './api';
import { BRIDGE_EVENTS, initial, reducer, type Action, type State } from './store';

const BRIDGE_URL = process.env.NEXT_PUBLIC_BRIDGE_URL ?? 'http://127.0.0.1:8787/events';

export type Link = 'connecting' | 'open' | 'down';
const Ctx = createContext<{ state: State; dispatch: Dispatch<Action>; link: Link } | null>(null);

// The stream has no backlog: after every (re)connect, ask the orchestrator for what we missed.
async function catchUp(dispatch: Dispatch<Action>) {
  const [turns, decisions, memory] = await Promise.allSettled([api.transcript(), api.decisions({ session: 'current' }), api.memory()]);
  const ok = <T,>(r: PromiseSettledResult<T[]>) => (r.status === 'fulfilled' ? r.value : undefined);
  dispatch({ type: 'hydrate', turns: ok(turns), decisions: ok(decisions), memory: ok(memory) });
}

export function BridgeProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);
  const [link, setLink] = useState<Link>('connecting');

  useEffect(() => {
    const es = new EventSource(BRIDGE_URL); // reconnects by itself
    es.onopen = () => { setLink('open'); catchUp(dispatch); };
    es.onerror = () => setLink('down');
    es.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data);
        if (BRIDGE_EVENTS.has(ev?.type)) dispatch(ev);
      } catch { /* not JSON: ignore the frame */ }
    };
    return () => es.close();
  }, []);

  return <Ctx.Provider value={{ state, dispatch, link }}>{children}</Ctx.Provider>;
}

export function useBridge() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error('useBridge needs <BridgeProvider>');
  return ctx;
}
