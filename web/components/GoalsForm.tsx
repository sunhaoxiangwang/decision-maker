'use client';
import { useState } from 'react';
import type { Option, Session } from '@/lib/store';
import { CUES, Dot } from './CueBadge';

// The composer: type a goal and press Enter to add it, then Confirm to send the session
// to the orchestrator. Any edit un-confirms the goals until Confirm is pressed again.
export default function GoalsForm({ session, save }: { session: Session; save: (next: Session, post: boolean) => void }) {
  const [draft, setDraft] = useState('');
  const edit = (patch: Partial<Session>) =>
    save({ ...session, ...patch, goals: (patch.goals ?? session.goals).map((g) => ({ ...g, confirmed: false })) }, false);

  const add = () => {
    const [key, ...rest] = draft.trim().split(/\s+/);
    edit({ goals: [...session.goals.filter((g) => g.key !== key), { key, value: rest.join(' '), confirmed: false }] });
    setDraft('');
  };
  const confirm = () => save({ ...session, goals: session.goals.map((g) => ({ ...g, confirmed: true })) }, true);
  const options = session.lunch_options ?? { A: '', B: '', C: '' };
  const confirmed = session.goals.length > 0 && session.goals.every((g) => g.confirmed);

  return (
    <form className="composer" onSubmit={(e) => { e.preventDefault(); draft.trim() ? add() : confirm(); }}>
      {session.mode === 'lunch' && (
        <div className="options">
          {(['A', 'B', 'C'] as Option[]).map((o) => (
            <label key={o}>
              <Dot color={CUES[`Lunch('${o}')`].color} /> {o}
              <input value={options[o]} placeholder={`Option ${o}`} onChange={(e) => edit({ lunch_options: { ...options, [o]: e.target.value } })} />
            </label>
          ))}
        </div>
      )}
      {session.goals.length > 0 && (
        <div className="goals">
          {session.goals.map((g) => (
            <span key={g.key} className="goal">
              {g.confirmed && '✓ '}{g.key} {g.value}
              <button type="button" aria-label={`Remove goal ${g.key}`} onClick={() => edit({ goals: session.goals.filter((x) => x !== g) })}>×</button>
            </span>
          ))}
        </div>
      )}
      <div className="row">
        <input
          aria-label="Add a goal" value={draft} onChange={(e) => setDraft(e.target.value)}
          placeholder={session.mode === 'lunch' ? 'Add a goal, e.g. "time_total ≤ 40 min"' : 'Add a goal, e.g. "budget_total ≤ 1200"'}
        />
        <button className="send" disabled={!draft.trim() && (confirmed || session.goals.length === 0)}>
          {draft.trim() ? 'Add' : confirmed ? 'Confirmed ✓' : 'Confirm goals'}
        </button>
      </div>
    </form>
  );
}
