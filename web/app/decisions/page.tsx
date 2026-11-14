'use client';
import Link from 'next/link';
import { useSearchParams } from 'next/navigation';
import { Suspense, useState } from 'react';
import CueBadge, { CUES } from '@/components/CueBadge';
import DecisionCard from '@/components/DecisionCard';
import { useBridge } from '@/lib/useBridge';

// Past decisions come from this browser's localStorage (lib/useBridge.tsx): every
// `decision` event received is kept there.
function Decisions() {
  const { state, dispatch } = useBridge();
  const memoryId = useSearchParams().get('memory');
  const [filter, setFilter] = useState({ mode: '', cue: '', day: '' });
  const [openId, setOpenId] = useState<string | null>(null);

  const all = Object.values(state.decisions);
  const rows = all
    .filter((d) => (!filter.mode || d.mode === filter.mode) && (!filter.cue || d.cue === filter.cue))
    .filter((d) => !filter.day || new Date(d.time_ms).toLocaleDateString('en-CA') === filter.day) // en-CA = YYYY-MM-DD, local time
    .filter((d) => !memoryId || d.memory_used.some((m) => m.id === memoryId))
    .sort((a, b) => b.time_ms - a.time_ms);
  const set = (k: keyof typeof filter) => (e: { target: { value: string } }) => setFilter({ ...filter, [k]: e.target.value });
  const open = openId ? state.decisions[openId] : undefined;
  const turn = open ? state.turns[open.turn_id] : undefined;

  return (
    <div className={open ? 'split' : ''}>
      <section className="page">
        <h1>Decisions</h1>
        <div className="filters">
          <select aria-label="Mode" value={filter.mode} onChange={set('mode')}>
            <option value="">All modes</option><option value="negotiation">Negotiation</option><option value="lunch">Lunch</option>
          </select>
          <select aria-label="Cue" value={filter.cue} onChange={set('cue')}>
            <option value="">All cues</option>
            {Object.entries(CUES).map(([cue, { label }]) => <option key={cue} value={cue}>{label}</option>)}
          </select>
          <input type="date" aria-label="Day" value={filter.day} onChange={set('day')} />
          {memoryId && <Link className="chip" href="/decisions">Using memory {state.memory[memoryId]?.text ?? memoryId} ×</Link>}
          {all.length > 0 && (
            <button className="ghost" onClick={() => confirm(`Delete all ${all.length} saved decisions from this browser?`) && dispatch({ type: 'forget-decisions' })}>
              Clear history
            </button>
          )}
        </div>
        {rows.length === 0 ? (
          <p className="muted">{all.length === 0 ? 'No decisions yet. They are saved in this browser as the agent makes them.' : 'No decisions match.'}</p>
        ) : (
          <div className="scroll-x">
            <table>
              <thead><tr><th>Time</th><th>Mode</th><th>Cue</th><th>Reason</th><th>Turn</th><th title="speech → decision → LED">Latency</th></tr></thead>
              <tbody>
                {rows.map((d) => (
                  <tr key={d.id} className={`pick${openId === d.id ? ' hl' : ''}`} onClick={() => setOpenId(d.id)}>
                    <td><button className="ghost">{new Date(d.time_ms).toLocaleString([], { dateStyle: 'short', timeStyle: 'medium' })}</button></td>
                    <td>{d.mode}</td>
                    <td><CueBadge cue={d.cue} late={d.shown === false} /></td>
                    <td>{d.reason}</td>
                    <td>t{d.turn_id}</td>
                    <td>{d.latency_ms.transcript_to_decision}{d.latency_ms.decision_to_led != null && ` → ${d.latency_ms.decision_to_led}`} ms</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      {open && (
        <aside className="pane">
          <header>
            <h2>Decision · turn {open.turn_id}</h2>
            <button className="ghost" onClick={() => setOpenId(null)}>Close</button>
          </header>
          {open.context && open.context.length > 0 && (
            <section>
              <h3>Leading up to it</h3>
              <ul>{open.context.map((t) => <li key={t.turn_id}><small>t{t.turn_id}</small> {t.text}</li>)}</ul>
            </section>
          )}
          <DecisionCard turn={turn?.decisionId === open.id ? turn : undefined} d={open} options={state.session.lunch_options} />
          <details>
            <summary>Raw agent output</summary>
            <pre>{JSON.stringify(open, null, 2)}</pre>
          </details>
        </aside>
      )}
    </div>
  );
}

export default function Page() {
  return <Suspense><Decisions /></Suspense>; // useSearchParams needs a Suspense boundary
}
