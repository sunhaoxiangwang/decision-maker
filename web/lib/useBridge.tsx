'use client';
// One EventSource for the whole app. Read directly from the bridge (never through a
// Next.js rewrite). The bridge only answers its BRIDGE_UI_ORIGIN, so open the UI there.
//
// Decisions, memory and the session's goals are stored in this browser's localStorage:
// that is where the Decisions and Memory tabs read the past from.
// ponytail: localStorage holds about 5 MB and is per browser; move to the orchestrator's
// store (or IndexedDB) when history outgrows that or must be shared.
import { createContext, useContext, useEffect, useReducer, useState, type Dispatch, type ReactNode } from 'react';
import { BRIDGE_EVENTS, initial, reducer, type Action, type Saved, type State } from './store';

const BRIDGE_URL = process.env.NEXT_PUBLIC_BRIDGE_URL ?? 'http://127.0.0.1:8787/events';
const KEY = 'decision-maker:v1';

export type Link = 'connecting' | 'open' | 'down';
const Ctx = createContext<{ state: State; dispatch: Dispatch<Action>; link: Link } | null>(null);

function readSaved(): Partial<Saved> {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? '{}');
    return v && typeof v === 'object' ? v : {};
  } catch {
    return {}; // storage blocked or corrupt: start empty
  }
}

export function BridgeProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);
  const [link, setLink] = useState<Link>('connecting');

  useEffect(() => {
    dispatch({ type: 'load', saved: readSaved() });
    const es = new EventSource(BRIDGE_URL); // reconnects by itself
    es.onopen = () => setLink('open');
    es.onerror = () => setLink('down');
    es.onmessage = (e) => {
      try {
        const ev = JSON.parse(e.data);
        if (BRIDGE_EVENTS.has(ev?.type)) dispatch(ev);
      } catch { /* not JSON: ignore the frame */ }
    };
    return () => es.close();
  }, []);

  const { loaded, decisions, memory, session } = state;
  useEffect(() => {
    if (!loaded) return; // never overwrite the saved copy before it has been read
    try { localStorage.setItem(KEY, JSON.stringify({ decisions, memory, session } satisfies Saved)); } catch { /* full or blocked */ }
  }, [loaded, decisions, memory, session]);

  return <Ctx.Provider value={{ state, dispatch, link }}>{children}</Ctx.Provider>;
}

export function useBridge() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error('useBridge needs <BridgeProvider>');
  return ctx;
}
