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

// The copilot's three decisions under the band colour each one lights (docs/contract-v1.md).
const SCALE = [
  { cue: 'Proceed', colour: 'Green', color: 'var(--green)' },
  { cue: 'Pause', colour: 'Red', color: 'var(--red)' },
  { cue: 'Ask', colour: 'Amber', color: 'var(--amber)' },
] as const;

export function CueScale({ cue }: { cue: Cue }) {
  return (
    <ol className="scale" aria-label="Copilot decision">
      {SCALE.map((s) => (
        <li key={s.cue} className={s.cue === cue ? 'on' : ''} aria-current={s.cue === cue ? 'true' : undefined}
          style={{ ['--c' as string]: s.color }}>
          <span className="swatch">{s.colour}</span>
          <span className="verdict">{s.cue}</span>
        </li>
      ))}
    </ol>
  );
}
