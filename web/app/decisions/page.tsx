'use client';
import Link from 'next/link';
import { useSearchParams } from 'next/navigation';
import { Suspense, useEffect, useState } from 'react';
import CueBadge, { CUES } from '@/components/CueBadge';
import DecisionCard from '@/components/DecisionCard';
import { api } from '@/lib/api';
import type { DecisionEvent } from '@/lib/store';
import { useBridge } from '@/lib/useBridge';

function Decisions() {
  const { state } = useBridge();
  const memoryId = useSearchParams().get('memory');
  const [filter, setFilter] = useState({ mode: '', cue: '', session: '' });
  const [remote, setRemote] = useState<DecisionEvent[] | null>(null); // null = orchestrator not reachable
  const [open, setOpen] = useState<DecisionEvent | null>(null);
  const [raw, setRaw] = useState<unknown>(null);
  const liveCount = Object.keys(state.decisions).length;

  useEffect(() => {
    let current = true;
    api.decisions(filter).then((r) => current && setRemote(r), () => current && setRemote(null));
    return () => { current = false; };
  }, [filter, liveCount]); // refetch when a new decision arrives on the stream

  useEffect(() => {
    setRaw(null);
    if (!open) return;
    let current = true;
    api.decision(open.id).then((r) => current && setRaw(r), () => {});
    return () => { current = false; };
  }, [open]);

  // Fallback: this tab's decisions from the event stream (which knows no sessions).
  const local = Object.values(state.decisions).filter((d) => (!filter.mode || d.mode === filter.mode) && (!filter.cue || d.cue === filter.cue));
  const rows = (remote ?? local)
    .filter((d) => !memoryId || d.memory_used.some((m) => m.id === memoryId))
    .sort((a, b) => b.time_ms - a.time_ms);
  const set = (k: keyof typeof filter) => (e: { target: { value: string } }) => setFilter({ ...filter, [k]: e.target.value });
  const turn = open ? state.turns[open.turn_id] : undefined;
  const before = open && turn?.decisionId === open.id
    ? Object.values(state.turns).filter((t) => t.turn_id <= open.turn_id).sort((a, b) => a.turn_id - b.turn_id).slice(-4)
    : [];

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
          <input aria-label="Session" placeholder="Session id" value={filter.session} onChange={set('session')} disabled={!remote} />
          {memoryId && <Link className="chip" href="/decisions">Using memory {memoryId} ×</Link>}
          {state.mock && <span className="chip mock">Mock data</span>}
        </div>
        {!remote && <p className="notice">Orchestrator history isn't reachable, so this shows only the decisions received in this tab.</p>}
        {rows.length === 0 ? <p className="muted">No decisions match.</p> : (
          <div className="scroll-x">
            <table>
              <thead><tr><th>Time</th><th>Mode</th><th>Cue</th><th>Reason</th><th>Turn</th><th title="speech → decision → LED">Latency</th></tr></thead>
              <tbody>
                {rows.map((d) => (
                  <tr key={d.id} className={`pick${open?.id === d.id ? ' hl' : ''}`} onClick={() => setOpen(d)}>
                    <td><button className="ghost">{new Date(d.time_ms).toLocaleTimeString()}</button></td>
                    <td>{d.mode}</td>
                    <td><CueBadge cue={d.cue} /></td>
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
            <button className="ghost" onClick={() => setOpen(null)}>Close</button>
          </header>
          {before.length > 0 && (
            <section>
              <h3>Leading up to it</h3>
              <ul>{before.map((t) => <li key={t.turn_id}><small>t{t.turn_id}</small> {t.text}</li>)}</ul>
            </section>
          )}
          <DecisionCard turn={turn?.decisionId === open.id ? turn : undefined} d={open} options={state.session.lunch_options} />
          <details>
            <summary>Raw agent output</summary>
            <pre>{JSON.stringify(raw ?? open, null, 2)}</pre>
          </details>
        </aside>
      )}
    </div>
  );
}

export default function Page() {
  return <Suspense><Decisions /></Suspense>; // useSearchParams needs a Suspense boundary
}
