'use client';
import { useEffect, useRef, useState } from 'react';
import { selectedTurn, type State } from '@/lib/store';
import CueBadge, { Dot } from './CueBadge';

const COLORS = ['#10a37f', '#5436da', '#e2622b', '#d6336c', '#0b84d8', '#8a7300'];

// Speaker names are a per-viewer convenience, so localStorage is enough.
function useNames() {
  const [names, setNames] = useState<Record<string, string>>({});
  useEffect(() => {
    try { setNames(JSON.parse(localStorage.getItem('speakerNames') ?? '{}') || {}); } catch { /* storage blocked */ }
  }, []);
  const rename = (key: string, name: string) => {
    const next = { ...names, [key]: name };
    setNames(next);
    try { localStorage.setItem('speakerNames', JSON.stringify(next)); } catch { /* storage blocked */ }
  };
  return [names, rename] as const;
}

export default function Conversation({ state, onPick }: { state: State; onPick: (turn_id: number) => void }) {
  const [names, rename] = useNames();
  const end = useRef<HTMLDivElement>(null);
  const turns = Object.values(state.turns).sort((a, b) => a.turn_id - b.turn_id);
  const last = turns.at(-1);
  const selected = selectedTurn(state);

  useEffect(() => { end.current?.scrollIntoView({ block: 'end' }); }, [turns.length, last?.text]);

  return (
    <div className="col">
      {turns.map((t) => {
        const d = t.decisionId ? state.decisions[t.decisionId] : undefined;
        const cue = t.cue ?? d?.cue;
        const key = String(t.speaker ?? 'unknown');
        const fallback = t.speaker == null ? 'Unknown speaker' : `Speaker ${t.speaker + 1}`;
        const name = names[key] ?? '';
        return (
          <article key={t.turn_id} className={`turn${selected === t.turn_id ? ' sel' : ''}`}>
            <div className="who">
              <Dot color={COLORS[(t.speaker ?? COLORS.length - 1) % COLORS.length]} />
              <input
                aria-label={`Name for ${fallback}`} title="Click to rename"
                value={name} placeholder={fallback} size={Math.max((name || fallback).length, 4)}
                onChange={(e) => rename(key, e.target.value)}
              />
              <span className="muted">t{t.turn_id}</span>
            </div>
            {t.text && (
              <button className={`bubble${t.final ? '' : ' interim'}`} onClick={() => onPick(t.turn_id)}>
                {t.text}
              </button>
            )}
            {cue && (
              <button className="reply" onClick={() => onPick(t.turn_id)}>
                <CueBadge cue={cue} late={t.shown === false} />
                <span>{t.shown === false ? 'late — not sent to band' : d?.reason}</span>
              </button>
            )}
          </article>
        );
      })}
      <div ref={end} />
    </div>
  );
}
