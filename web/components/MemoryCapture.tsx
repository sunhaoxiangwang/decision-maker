'use client';
import type { State } from '@/lib/store';

export default function MemoryCapture({ state }: { state: State }) {
  const c = state.context;
  if (!c) return null;
  return (
    <div className="context-confirm" role="status">
      <b>Memory capture on · wave again to stop</b>
      <p className="muted">Finished transcript turns are saved as given in this browser’s Memory.</p>
      <p style={{ whiteSpace: 'pre-wrap' }}>{c.text || 'Listening after the white flash…'}</p>
      {c.interim && <p className="muted">{c.interim}</p>}
    </div>
  );
}
