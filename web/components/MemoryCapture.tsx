'use client';
import type { State } from '@/lib/store';

// Shown while a wave capture is running: what has been heard so far, live.
export default function MemoryCapture({ state }: { state: State }) {
  const c = state.context;
  if (!c) return null;
  const lines = c.text ? c.text.split('\n') : [];
  return (
    <section className="recorder" role="status" aria-live="polite">
      <header>
        <span className="rec-dot" aria-hidden />
        <b>Recording memory</b>
        <span className="muted">Wave again to stop</span>
      </header>
      {lines.length === 0 && !c.interim && <p className="muted">Listening. Start speaking after the white flash.</p>}
      {lines.map((l, i) => <p key={i} className="heard">{l}</p>)}
      {c.interim && <p className="heard interim">{c.interim}</p>}
      <p className="fine">Saved word for word as a proposal. Confirm it in Memory; speech never confirms it.</p>
    </section>
  );
}
