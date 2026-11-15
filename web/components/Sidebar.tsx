'use client';
import Link from 'next/link';
import { usePathname } from 'next/navigation';
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
  const { state, dispatch, link, storageError } = useBridge();
  return (
    <nav className="side">
      <div className="brand">Decision Maker</div>
      {TABS.map(([href, label]) => (
        <Link key={href} href={href} className={path === href ? 'on' : ''} aria-current={path === href ? 'page' : undefined}>{label}</Link>
      ))}
      <div className="foot">
        <span className="connection"><Dot color={LINK[link][0]} /> {LINK[link][1]}</span>
        {storageError && <p role="alert">{storageError}</p>}
        {state.notice && <div className="side-notice" role="alert"><p>{state.notice}</p><button className="ghost" onClick={() => dispatch({ type: 'dismiss-notice' })}>Dismiss</button></div>}
        {state.status && <span className="muted">Band {state.status.band}</span>}
      </div>
    </nav>
  );
}
