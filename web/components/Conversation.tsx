'use client';
import { useEffect, useRef } from 'react';
import { fallbackName, useNames } from '@/lib/names';
import { selectedTurn, visibleTurns, type State } from '@/lib/store';
import CueBadge, { Dot } from './CueBadge';

const COLORS = ['#10a37f', '#5436da', '#e2622b', '#d6336c', '#0b84d8', '#8a7300'];

export default function Conversation({ state, onPick }: { state: State; onPick: (turn_id: number) => void }) {
  const { names, rename } = useNames();
  const end = useRef<HTMLDivElement>(null);
  const turns = visibleTurns(state);
  const last = turns.at(-1);
  const selected = selectedTurn(state);

  useEffect(() => { end.current?.scrollIntoView({ block: 'end' }); }, [turns.length, last?.text]);

  return (
    <div className="col">
      {turns.map((t, i) => {
        const d = t.decisionId ? state.decisions[t.decisionId] : undefined;
        const cue = t.cue ?? d?.cue;
        const key = String(t.speaker ?? 'unknown');
        const fallback = fallbackName(t.speaker);
        const name = names[key] ?? '';
        // Speech arrives as one turn per pause, so one person often fills several turns in
        // a row: name them once, until someone else speaks or the agent answers.
        const prev = turns[i - 1];
        const grouped = prev !== undefined && prev.speaker === t.speaker && !prev.cue && !prev.decisionId;
        return (
          <article key={t.turn_id} className={`turn${selected === t.turn_id ? ' sel' : ''}${grouped ? ' grouped' : ''}`}>
            <div className="who">
              {!grouped && (
                <>
                  <Dot color={COLORS[(t.speaker ?? COLORS.length - 1) % COLORS.length]} />
                  <input
                    aria-label={`Name for ${fallback}`} title="Click to rename"
                    value={name} placeholder={fallback} size={Math.max((name || fallback).length, 4)}
                    onChange={(e) => rename(key, e.target.value)}
                  />
                </>
              )}
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
