'use client';
import { useEffect, useState } from 'react';
import MemoryList from '@/components/MemoryList';
import { api } from '@/lib/api';
import type { MemoryEvent, MemoryItem } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

export default function MemoryPage() {
  const { state, dispatch } = useBridge();
  const [error, setError] = useState('');
  const [highlight, setHighlight] = useState('');

  useEffect(() => {
    api.memory().then((memory) => dispatch({ type: 'hydrate', memory }), () => {});
    const id = decodeURIComponent(location.hash.replace(/^#m-/, '')); // linked from a decision
    setHighlight(id);
    document.getElementById(`m-${id}`)?.scrollIntoView({ block: 'center' });
  }, [dispatch]);

  // Change the store only after the orchestrator accepted the change. Mock data has no
  // orchestrator behind it, so there the change is applied to this tab only.
  async function change(op: MemoryEvent['op'], item: MemoryItem, request: Promise<unknown>) {
    try {
      await request;
    } catch (e) {
      if (!state.mock) return setError(`Not saved (${e instanceof Error ? e.message : e}). Is the orchestrator running?`);
    }
    setError('');
    dispatch({ type: 'memory', op, item });
  }

  const items = Object.values(state.memory).sort((a, b) => b.saved_ms - a.saved_ms);
  return (
    <section className="page">
      <h1>Memory {state.mock && <span className="chip mock">Mock data</span>}</h1>
      {error && <p className="notice" role="alert">{error}</p>}
      <h2>Confirmed</h2>
      <p className="muted">Preferences and facts the agent may rely on.</p>
      <MemoryList
        items={items.filter((m) => m.status === 'confirmed')} highlight={highlight}
        onSave={(m, value) => value !== m.value && change('updated', { ...m, value }, api.patchMemory(m.id, { value }))}
        onDelete={(m) => confirm(`Delete "${m.key}"?`) && change('deleted', m, api.deleteMemory(m.id))}
      />
      <h2>Proposed</h2>
      <p className="muted">Suggested by post-session summaries. Never used for a decision until you confirm it.</p>
      <MemoryList
        items={items.filter((m) => m.status === 'proposed')} highlight={highlight}
        onConfirm={(m) => change('confirmed', { ...m, status: 'confirmed' }, api.confirmMemory(m.id))}
        onDelete={(m) => change('deleted', m, api.deleteMemory(m.id))}
      />
    </section>
  );
}
