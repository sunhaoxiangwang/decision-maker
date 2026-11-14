import Link from 'next/link';
import { ago, type MemoryItem } from '@/lib/store';

type Props = {
  items: MemoryItem[];
  uses: Record<string, number>;                       // saved decisions that used each note
  highlight?: string;
  onSave?: (item: MemoryItem, text: string) => void;  // confirmed notes: edit the sentence
  onConfirm?: (item: MemoryItem) => void;             // proposed notes
  onDelete: (item: MemoryItem) => void;               // delete, or reject a proposal
};

const source = (m: MemoryItem) =>
  m.source.type === 'user' ? 'typed by you' : `conversation${m.source.turn_ids.length ? ` · t${m.source.turn_ids.join(', t')}` : ''}`;

export default function MemoryList({ items, uses, highlight, onSave, onConfirm, onDelete }: Props) {
  if (items.length === 0) return <p className="muted">Nothing here yet.</p>;
  return (
    <div className="scroll-x">
      <table>
        <thead>
          <tr><th>Kind</th><th>About</th><th>Memory</th><th>Source</th><th>Saved</th><th>Used by</th><th /></tr>
        </thead>
        <tbody>
          {items.map((m) => {
            const n = uses[m.id] ?? 0;
            return (
              <tr key={m.id} id={`m-${m.id}`} className={highlight === m.id ? 'hl' : ''}>
                <td><span className={`tag${m.kind === 'constraint' ? ' red' : m.kind === 'commitment' ? ' amber' : ''}`}>{m.kind}</span></td>
                <td>{m.subject}</td>
                <td className="note">
                  {onSave ? (
                    <form onSubmit={(e) => { e.preventDefault(); onSave(m, String(new FormData(e.currentTarget).get('text') ?? '')); }}>
                      {/* key: reset the field when the stored text changes */}
                      <input key={m.text} name="text" defaultValue={m.text} aria-label={`Memory about ${m.subject}`} title="Edit, then press Enter to save" />
                    </form>
                  ) : m.text}
                  {m.keywords.length > 0 && <small> {m.keywords.join(' · ')}</small>}
                </td>
                <td><small>{source(m)}</small></td>
                <td>{ago(m.saved_ms)}</td>
                <td><Link href={`/decisions?memory=${encodeURIComponent(m.id)}`}>{n} decision{n === 1 ? '' : 's'}</Link></td>
                <td className="actions">
                  {onConfirm && <button className="send" onClick={() => onConfirm(m)}>Confirm</button>}
                  <button className="ghost" onClick={() => onDelete(m)}>{onConfirm ? 'Reject' : 'Delete'}</button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
