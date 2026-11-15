'use client';
import { useEffect, useRef } from 'react';
import { selectedTurn, visibleTurns, type State } from '@/lib/store';

// Speech only: the band wearer (speaker 0) on the right in a darker bubble, everyone else on
// the left. The copilot's decision is shown in the right panel, never in the chat.
export default function Conversation({ state, onPick }: { state: State; onPick: (turn_id: number) => void }) {
  const end = useRef<HTMLDivElement>(null);
  const turns = visibleTurns(state);
  const last = turns.at(-1);
  const selected = selectedTurn(state);

  useEffect(() => { end.current?.scrollIntoView({ block: 'end' }); }, [turns.length, last?.text]);

  return (
    <div className="col convo">
      {turns.map((t) => {
        return (
          <div key={t.turn_id} className="exchange">
            {t.text && (
              <button className={`bubble ${t.speaker === 0 ? 'you' : 'others'}${t.final ? '' : ' interim'}${selected === t.turn_id ? ' sel' : ''}`}
                onClick={() => onPick(t.turn_id)}>
                {t.text}
              </button>
            )}
          </div>
        );
      })}
      <div ref={end} />
    </div>
  );
}
