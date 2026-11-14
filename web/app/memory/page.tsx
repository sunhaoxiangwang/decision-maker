'use client';
import { useEffect, useState } from 'react';
import MemoryList from '@/components/MemoryList';
import type { MemoryEvent, MemoryItem } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

// Memory lives in this browser's localStorage (lib/useBridge.tsx). Items arrive as `memory`
// events from the stream, or are added here by the user.
export default function MemoryPage() {
  const { state, dispatch } = useBridge();
  const [highlight, setHighlight] = useState('');

  useEffect(() => {
    if (!state.loaded) return;
    const id = decodeURIComponent(location.hash.replace(/^#m-/, '')); // linked from a decision
    setHighlight(id);
    document.getElementById(`m-${id}`)?.scrollIntoView({ block: 'center' });
  }, [state.loaded]);

  const change = (op: MemoryEvent['op'], item: MemoryItem) => dispatch({ type: 'memory', op, item });
  const items = Object.values(state.memory).sort((a, b) => b.saved_ms - a.saved_ms);
  const uses: Record<string, number> = {};
  for (const d of Object.values(state.decisions)) for (const m of d.memory_used) uses[m.id] = (uses[m.id] ?? 0) + 1;

  function add(form: HTMLFormElement) {
    const data = new FormData(form);
    const key = String(data.get('key') ?? '').trim();
    const value = String(data.get('value') ?? '').trim();
    if (!key || !value) return;
    change('confirmed', { id: crypto.randomUUID(), key, value, status: 'confirmed', source: 'user', saved_ms: Date.now(), used_by: 0 });
    form.reset();
  }

  return (
    <section className="page">
      <h1>Memory</h1>
      <p className="muted">Stored in this browser only.</p>
      <h2>Confirmed</h2>
      <p className="muted">Preferences and facts the agent may rely on.</p>
      <MemoryList
        items={items.filter((m) => m.status === 'confirmed')} uses={uses} highlight={highlight}
        onSave={(m, value) => value !== m.value && change('updated', { ...m, value })}
        onDelete={(m) => confirm(`Delete "${m.key}"?`) && change('deleted', m)}
      />
      <form className="filters add" onSubmit={(e) => { e.preventDefault(); add(e.currentTarget); }}>
        <input name="key" aria-label="Key" placeholder="Key, e.g. budget_total" required />
        <input name="value" aria-label="Value" placeholder="Value, e.g. ≤ $1,200" required />
        <button className="send">Add</button>
      </form>
      <h2>Proposed</h2>
      <p className="muted">Suggested by post-session summaries. Never used for a decision until you confirm it.</p>
      <MemoryList
        items={items.filter((m) => m.status === 'proposed')} uses={uses} highlight={highlight}
        onConfirm={(m) => change('confirmed', { ...m, status: 'confirmed' })}
        onDelete={(m) => change('deleted', m)}
      />
    </section>
  );
}
