'use client';
import { useEffect, useState } from 'react';
import MemoryList from '@/components/MemoryList';
import { extractMemory } from '@/lib/api';
import { asPrompt, forDecision, transcriptLines, words } from '@/lib/memory';
import { useNames } from '@/lib/names';
import { ago, type Conversation, type MemoryEvent, type MemoryItem } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

// Memory is written as sentences, two ways: from context the user types here, and from past
// conversations (OpenAI, through app/llm/memory/route.ts). Everything is kept in this
// browser's localStorage (lib/useBridge.tsx) and sent to the orchestrator when it changes.
export default function MemoryPage() {
  const { state, dispatch } = useBridge();
  const { name } = useNames();
  const [highlight, setHighlight] = useState('');
  const [busy, setBusy] = useState<string | null>(null); // 'context' or a conversation id
  const [notice, setNotice] = useState('');

  useEffect(() => {
    if (!state.loaded) return;
    const id = decodeURIComponent(location.hash.replace(/^#m-/, '')); // linked from a decision
    setHighlight(id);
    document.getElementById(`m-${id}`)?.scrollIntoView({ block: 'center' });
  }, [state.loaded]);

  const change = (op: MemoryEvent['op'], item: MemoryItem) => dispatch({ type: 'memory', op, item });
  const items = Object.values(state.memory).sort((a, b) => b.saved_ms - a.saved_ms);
  const confirmed = items.filter((m) => m.status === 'confirmed');
  const uses: Record<string, number> = {};
  for (const d of Object.values(state.decisions)) for (const m of d.memory_used) uses[m.id] = (uses[m.id] ?? 0) + 1;
  const conversations = Object.values(state.conversations).sort((a, b) => b.started_ms - a.started_ms);

  async function addContext(form: HTMLFormElement) {
    const text = String(new FormData(form).get('context') ?? '').trim();
    if (!text) return;
    setBusy('context');
    setNotice('');
    try {
      const notes = await extractMemory({ source: 'user', text });
      notes.forEach((m) => change('confirmed', m));
      setNotice(notes.length ? '' : 'Nothing in that text looked worth remembering.');
    } catch (e) {
      // No model: keep each line as it was typed, so typed context is never lost.
      const now = Date.now();
      text.split('\n').map((l) => l.trim()).filter(Boolean).forEach((line) => change('confirmed', {
        id: crypto.randomUUID(), status: 'confirmed', kind: 'context', subject: 'user', text: line,
        keywords: [...new Set(words(line))].slice(0, 8), source: { type: 'user' }, saved_ms: now,
      }));
      setNotice(`Saved as typed, one note per line. (${(e as Error).message})`);
    }
    form.reset();
    setBusy(null);
  }

  async function extract(c: Conversation) {
    setBusy(c.id);
    setNotice('');
    try {
      const notes = await extractMemory({ source: 'conversation', conversation_id: c.id, transcript: transcriptLines(c, name) });
      notes.forEach((m) => change('proposed', m));
      dispatch({ type: 'extracted', id: c.id, time_ms: Date.now() });
      setNotice(notes.length ? `${notes.length} proposed note${notes.length === 1 ? '' : 's'}: confirm the ones that are right.` : 'Nothing in that conversation looked worth remembering.');
    } catch (e) {
      setNotice((e as Error).message);
    }
    setBusy(null);
  }

  return (
    <section className="page">
      <h1>Memory</h1>
      <p className="muted">What the decision agent may rely on, written as sentences. Stored in this browser and sent to the orchestrator.</p>

      <form className="context" onSubmit={(e) => { e.preventDefault(); addContext(e.currentTarget); }}>
        <textarea
          name="context" rows={3} aria-label="Context to remember"
          placeholder={'Tell it what to remember for this negotiation, e.g. "Queen, firm mattress. At most $1,200 including delivery. I need at least a 100-night return."'}
        />
        <button className="send" disabled={busy === 'context'}>{busy === 'context' ? 'Saving…' : 'Remember'}</button>
      </form>
      {notice && <p className="notice" role="status">{notice}</p>}

      <h2>Proposed</h2>
      <p className="muted">Taken from past conversations. Never used for a decision until you confirm it.</p>
      <MemoryList
        items={items.filter((m) => m.status === 'proposed')} uses={uses} highlight={highlight}
        onConfirm={(m) => change('confirmed', { ...m, status: 'confirmed' })}
        onDelete={(m) => change('deleted', m)}
      />

      <h2>Confirmed</h2>
      <p className="muted">Constraints and your own context are given to every decision; the rest when a turn mentions them.</p>
      <MemoryList
        items={confirmed} uses={uses} highlight={highlight}
        onSave={(m, text) => text.trim() && text !== m.text && change('updated', { ...m, text: text.trim(), keywords: m.keywords })}
        onDelete={(m) => confirm(`Delete "${m.text}"?`) && change('deleted', m)}
      />

      <h2>Past conversations</h2>
      <p className="muted">Every finished turn is kept here. Remember a conversation to propose notes from it.</p>
      {conversations.length === 0 && <p className="muted">None yet. They are saved as people speak.</p>}
      {conversations.map((c) => {
        const speakers = [...new Set(c.turns.map((t) => name(t.speaker)))];
        return (
          <details key={c.id} className="conv">
            <summary>
              {new Date(c.started_ms).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })} · {c.mode} · {c.turns.length} turn{c.turns.length === 1 ? '' : 's'} · {speakers.join(', ')}
              {c.id === state.current && ' · live'}
              {c.extracted_ms && ` · remembered ${ago(c.extracted_ms)}`}
            </summary>
            <ul>{c.turns.map((t) => <li key={t.turn_id}><small>t{t.turn_id}</small> <b>{name(t.speaker)}</b> {t.text}</li>)}</ul>
            <div className="filters">
              <button className="send" disabled={busy === c.id || c.turns.length === 0} onClick={() => extract(c)}>
                {busy === c.id ? 'Reading…' : c.extracted_ms ? 'Remember again' : 'Remember this conversation'}
              </button>
              <button className="ghost" onClick={() => confirm('Delete this conversation from this browser? Its memory notes stay.') && dispatch({ type: 'delete-conversation', id: c.id })}>
                Delete
              </button>
            </div>
          </details>
        );
      })}

      <details>
        <summary>As the decision API receives it ({confirmed.length} confirmed note{confirmed.length === 1 ? '' : 's'})</summary>
        <pre>{JSON.stringify(forDecision(confirmed), null, 2)}</pre>
        <pre>{asPrompt(confirmed) || '(no confirmed memory)'}</pre>
      </details>
    </section>
  );
}
