'use client';
import { useEffect, useRef } from 'react';
import { selectedTurn, visibleTurns, type State } from '@/lib/store';
import CueBadge, { CUES, CueScale } from './CueBadge';

// Speech on the right (the band wearer, speaker 0, in a darker bubble); the copilot's answer
// on the left: what it was given (collapsed), then its decision under the band colours.
export default function Conversation({ state, onPick }: { state: State; onPick: (turn_id: number) => void }) {
  const end = useRef<HTMLDivElement>(null);
  const turns = visibleTurns(state);
  const last = turns.at(-1);
  const selected = selectedTurn(state);

  useEffect(() => { end.current?.scrollIntoView({ block: 'end' }); }, [turns.length, last?.text, last?.cue]);

  return (
    <div className="col convo">
      {turns.map((t) => {
        const d = t.decisionId ? state.decisions[t.decisionId] : undefined;
        const cue = t.cue ?? d?.cue;
        return (
          <div key={t.turn_id} className="exchange">
            {t.text && (
              <button className={`bubble ${t.speaker === 0 ? 'you' : 'others'}${t.final ? '' : ' interim'}${selected === t.turn_id ? ' sel' : ''}`}
                onClick={() => onPick(t.turn_id)}>
                {t.text}
              </button>
            )}
            {cue && (
              <article className="agent" style={{ ['--cue' as string]: CUES[cue]?.color ?? 'var(--line)' }}>
                {d && (d.context?.length || d.memory_used.length > 0) ? (
                  <details className="agent-context">
                    <summary>Context</summary>
                    {d.context && d.context.length > 0 && (
                      <ul>{d.context.map((c) => <li key={c.turn_id}>{c.text}</li>)}</ul>
                    )}
                    {d.memory_used.length > 0 && (
                      <>
                        <p className="muted">Memory used</p>
                        <ul>{d.memory_used.map((m) => <li key={m.id}>{m.text}</li>)}</ul>
                      </>
                    )}
                  </details>
                ) : null}
                {['Proceed', 'Pause', 'Ask'].includes(cue) ? <CueScale cue={cue} /> : <CueBadge cue={cue} late={t.shown === false} />}
                {t.shown === false && <p className="muted">Arrived late, so the band was not lit.</p>}
                {d?.reason && <p>{d.reason}</p>}
                {d && (d.conflicts.length > 0 || d.missing.length > 0) && (
                  <div className="tags">
                    {d.conflicts.map((c) => <span key={c} className="tag red">{c}</span>)}
                    {d.missing.map((m) => <span key={m} className="tag amber">{m}</span>)}
                  </div>
                )}
                <button className="ghost" onClick={() => onPick(t.turn_id)}>Show details</button>
              </article>
            )}
          </div>
        );
      })}
      <div ref={end} />
    </div>
  );
}
