import type { Cue } from '@/lib/store';

// Screen colours for each cue (the band's own LED colours live in src/bridge.rs).
export const CUES: Record<Cue, { label: string; color: string }> = {
  Proceed: { label: 'Proceed', color: 'var(--green)' },
  Pause: { label: 'Pause', color: 'var(--red)' },
  Ask: { label: 'Ask', color: 'var(--amber)' },
  "Lunch('A')": { label: 'Option A', color: 'var(--green)' },
  "Lunch('B')": { label: 'Option B', color: 'var(--purple)' },
  "Lunch('C')": { label: 'Option C', color: 'var(--white)' },
  Off: { label: 'No cue', color: 'transparent' },
};
const meta = (cue: Cue) => CUES[cue] ?? { label: String(cue), color: 'transparent' };

export function Dot({ color }: { color: string }) {
  return <i className="dot" style={{ background: color }} aria-hidden />;
}

export default function CueBadge({ cue, late, big }: { cue: Cue; late?: boolean; big?: boolean }) {
  const { label, color } = meta(cue);
  return (
    <span className={`badge${big ? ' big' : ''}${late ? ' late' : ''}`}>
      <Dot color={color} />
      {label}
    </span>
  );
}
