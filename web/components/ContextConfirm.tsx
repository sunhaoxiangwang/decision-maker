'use client';
import { useState } from 'react';
import { rememberContext } from '@/lib/api';
import type { Action, State } from '@/lib/store';

// After a wave: shows the spoken context as it is heard, then asks to confirm it. Confirming
// replaces the user's own context notes (memory "typed by you" or spoken after a wave);
// notes taken from conversations stay. Decisions use the new notes from the next turn on.
export default function ContextConfirm({ state, dispatch }: { state: State; dispatch: (a: Action) => void }) {
  const [busy, setBusy] = useState(false);
  const [warning, setWarning] = useState('');
  const c = state.context;
  if (!c) return warning ? <p className="notice" role="status">{warning}</p> : null;
  const replaced = Object.values(state.memory).filter((m) => m.status === 'confirmed' && m.source.type === 'user').length;

  async function confirm(form: HTMLFormElement) {
    const text = String(new FormData(form).get('context') ?? '').trim();
    if (!text) return;
    setBusy(true);
    const { items, warning } = await rememberContext(text);
    dispatch({ type: 'replace-user-memory', items });
    dispatch({ type: 'context-done' });
    setWarning(warning ?? `Memory updated: ${items.length} note${items.length === 1 ? '' : 's'} replace ${replaced}.`);
    setBusy(false);
  }

  return (
    <form className="context-confirm" onSubmit={(e) => { e.preventDefault(); confirm(e.currentTarget); }}>
      <b>{c.listening ? '👋 Wave — say the updated context now…' : 'Replace your context with this?'}</b>
      {c.listening && !c.text ? (
        <p className="muted">{c.interim || 'Listening (the band flashes white, then turns blue).'}</p>
      ) : (
        // key: a new spoken turn resets the field to include it
        <textarea key={c.text} name="context" rows={2} defaultValue={c.text} aria-label="Spoken context, editable" />
      )}
      <div className="filters">
        <button className="send" disabled={busy || !c.text}>{busy ? 'Saving…' : `Confirm · replaces ${replaced} note${replaced === 1 ? '' : 's'}`}</button>
        <button type="button" className="ghost" onClick={() => { dispatch({ type: 'context-done' }); setWarning(''); }}>Discard</button>
      </div>
    </form>
  );
}
