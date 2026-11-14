import Link from 'next/link';
import type { ReactNode } from 'react';
import { ago, type Decision, type Option, type Session, type Turn } from '@/lib/store';
import CueBadge, { CUES, Dot } from './CueBadge';

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <h3>{title}</h3>
      {children}
    </section>
  );
}

export default function DecisionCard({ turn, d, options }: { turn?: Turn; d?: Decision; options?: Session['lunch_options'] }) {
  const cue = turn?.cue ?? d?.cue;
  if (!cue) return <p className="muted">No decision for this turn yet.</p>;
  const late = (turn?.shown ?? d?.shown) === false;
  return (
    <div className="card">
      <CueBadge cue={cue} late={late} big />
      {late && <p className="muted">late — not sent to band</p>}
      {!d && <p className="muted">The bridge sent this cue. Its reason, facts and memory show here once the orchestrator publishes <code>decision</code> events.</p>}
      {d && (
        <>
          <p className="reason">{d.reason}</p>
          {d.conflicts.length > 0 && (
            <Section title="Conflicts">
              <ul>{d.conflicts.map((c) => <li key={c}><span className="tag red">{c}</span></li>)}</ul>
            </Section>
          )}
          {d.missing.length > 0 && (
            <Section title="Missing">
              <ul>{d.missing.map((m) => <li key={m}><span className="tag amber">{m}</span></li>)}</ul>
            </Section>
          )}
          {d.choice && (
            <Section title="Options">
              <ul>
                {(['A', 'B', 'C'] as Option[]).map((o) => {
                  const ok = d.choice!.eligible.includes(o);
                  return (
                    <li key={o} className={ok ? '' : 'muted'}>
                      <Dot color={CUES[`Lunch('${o}')`].color} /> <b>{o}</b> {options?.[o]}
                      <small> · {d.choice!.option === o ? 'chosen' : ok ? 'eligible' : 'not eligible'}</small>
                    </li>
                  );
                })}
              </ul>
              {d.choice.option === null && <p className="muted">No option chosen, so no light is shown.</p>}
            </Section>
          )}
          <Section title="Facts used">
            {d.facts.length === 0 && <p className="muted">None.</p>}
            <ul>
              {d.facts.map((f) => (
                <li key={f.key}>
                  <code>{f.key} = {f.value}</code>
                  <small> · t{f.turn_id}{f.confirmed ? '' : ' · unconfirmed'}</small>
                </li>
              ))}
            </ul>
          </Section>
          <Section title="Memory used">
            {d.memory_used.length === 0 && <p className="muted">None.</p>}
            <ul>
              {d.memory_used.map((m) => (
                <li key={m.id}>
                  <Link href={`/memory#m-${encodeURIComponent(m.id)}`}>{m.key}</Link> {m.value}
                  <small> · {m.source} · retrieved {ago(m.retrieved_ms)}</small>
                </li>
              ))}
            </ul>
          </Section>
          <p className="muted">
            <small>
              speech → decision {d.latency_ms.transcript_to_decision} ms
              {d.latency_ms.decision_to_led != null && ` · decision → LED ${d.latency_ms.decision_to_led} ms`}
            </small>
          </p>
        </>
      )}
    </div>
  );
}
