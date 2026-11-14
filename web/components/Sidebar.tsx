'use client';
import Link from 'next/link';
import { usePathname } from 'next/navigation';
import { lunchDemo, negotiationDemo, type Demo } from '@/lib/fixtures';
import { useBridge } from '@/lib/useBridge';
import { Dot } from './CueBadge';

const TABS = [['/', 'Live'], ['/decisions', 'Decisions'], ['/memory', 'Memory']] as const;
const LINK = {
  open: ['var(--green)', 'Bridge connected'],
  connecting: ['var(--amber)', 'Connecting to bridge…'],
  down: ['var(--red)', 'Bridge offline, retrying'],
} as const;

export default function Sidebar() {
  const path = usePathname();
  const { state, dispatch, link } = useBridge();
  const load = (demo: Demo) => {
    dispatch({ type: 'reset', mock: true });
    dispatch({ type: 'session', session: demo.session });
    demo.events.forEach((e) => dispatch(e));
  };
  return (
    <nav className="side">
      <div className="brand">Decision Maker</div>
      {TABS.map(([href, label]) => (
        <Link key={href} href={href} className={path === href ? 'on' : ''} aria-current={path === href ? 'page' : undefined}>{label}</Link>
      ))}
      <h3>Mock data</h3>
      <button onClick={() => load(negotiationDemo())}>Negotiation demo</button>
      <button onClick={() => load(lunchDemo())}>Lunch demo</button>
      {state.mock && <button onClick={() => dispatch({ type: 'reset', mock: false })}>Clear mock data</button>}
      <div className="foot">
        <span><Dot color={LINK[link][0]} /> {LINK[link][1]}</span>
        {state.status && <span className="muted">Band {state.status.band}</span>}
      </div>
    </nav>
  );
}
