'use client';
// One EventSource for the whole app. Read directly from the bridge (never through a
// Next.js rewrite). The bridge only answers its BRIDGE_UI_ORIGIN, so open the UI there.
//
// Decisions, memory, past conversations and the session's goals are stored in this
// browser's localStorage: that is where the Decisions and Memory tabs read the past from.
// History and memory have explicit byte/count budgets (limits.ts). Persistence failures
// are visible; writes are batched so speech interims do not serialize all history.
import { createContext, useContext, useEffect, useReducer, useRef, useState, type Dispatch, type ReactNode } from 'react';
import { readSaved, writeSaved, SAVE_ERROR } from './storage';
import { BRIDGE_EVENTS, initial, reducer, type Action, type Saved, type State } from './store';

const BRIDGE_URL = process.env.NEXT_PUBLIC_BRIDGE_URL ?? 'http://127.0.0.1:8790/events';

export type Link = 'connecting' | 'open' | 'down';
const Ctx = createContext<{ state: State; dispatch: Dispatch<Action>; link: Link; storageError: string | null } | null>(null);

export function BridgeProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);
  const [link, setLink] = useState<Link>('connecting');
  const [storageError, setStorageError] = useState<string | null>(null);
  const storageBlocked = useRef(false);

  useEffect(() => {
    try { dispatch({ type: 'load', saved: readSaved(localStorage) }); }
    catch (e) {
      storageBlocked.current = true; // never overwrite an unreadable saved copy
      setStorageError(`History could not be loaded: ${(e as Error).message}`);
      dispatch({ type: 'load', saved: {} });
    }
    const es = new EventSource(BRIDGE_URL); // reconnects by itself
    es.onopen = () => setLink('open');
    es.onerror = () => setLink('down');
    es.onmessage = (e) => {
      try {
        if (new TextEncoder().encode(e.data).byteLength > 64 * 1024) {
          dispatch({ type: 'gap', message: 'Bridge event exceeded the byte limit and was rejected.' });
          return;
        }
        const ev = JSON.parse(e.data);
        if (BRIDGE_EVENTS.has(ev?.type)) dispatch(ev);
      } catch { /* not JSON: ignore the frame */ }
    };
    return () => es.close();
  }, []);

  const { loaded, decisions, memory, session, conversations, current } = state;
  const pending = useRef<Partial<Saved>>({});
  const flush = () => {
    if (storageBlocked.current) return;
    try {
      setStorageError(writeSaved(localStorage, pending.current));
    } catch {
      setStorageError(SAVE_ERROR);
    }
  };
  const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (!loaded) return;
    pending.current = { decisions, memory, session, conversations, current } satisfies Saved;
    if (saveTimer.current === null) saveTimer.current = setTimeout(() => {
      saveTimer.current = null;
      flush();
    }, 500); // batch, with a maximum 500-ms delay rather than an endlessly reset debounce
  }, [loaded, decisions, memory, session, conversations, current]);
  useEffect(() => {
    const save = () => { if (saveTimer.current !== null) flush(); };
    window.addEventListener('pagehide', save);
    return () => {
      window.removeEventListener('pagehide', save);
      if (saveTimer.current !== null) { clearTimeout(saveTimer.current); flush(); }
    };
  }, []);
  // This branch has no decision/memory server. Keep memory explicitly local instead of
  // issuing requests to the proposed port 8788 and silently discarding their errors.

  return <Ctx.Provider value={{ state, dispatch, link, storageError }}>{children}</Ctx.Provider>;
}

export function useBridge() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error('useBridge needs <BridgeProvider>');
  return ctx;
}
