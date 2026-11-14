import Link from 'next/link';
import { ago, type MemoryItem } from '@/lib/store';

type Props = {
  items: MemoryItem[];
  uses: Record<string, number>;                       // saved decisions that used each item
  highlight?: string;
  onSave?: (item: MemoryItem, value: string) => void; // confirmed items: edit the value
  onConfirm?: (item: MemoryItem) => void;             // proposed items
  onDelete: (item: MemoryItem) => void;               // delete, or reject a proposal
};

export default function MemoryList({ items, uses, highlight, onSave, onConfirm, onDelete }: Props) {
  if (items.length === 0) return <p className="muted">Nothing here yet.</p>;
  return (
    <div className="scroll-x">
      <table>
        <thead>
          <tr><th>Key</th><th>Value</th><th>Source</th><th>Saved</th><th>Last used</th><th>Used by</th><th /></tr>
        </thead>
        <tbody>
          {items.map((m) => {
            const n = uses[m.id] ?? 0;
            return (
              <tr key={m.id} id={`m-${m.id}`} className={highlight === m.id ? 'hl' : ''}>
                <td><code>{m.key}</code></td>
                <td>
                  {onSave ? (
                    <form onSubmit={(e) => { e.preventDefault(); onSave(m, String(new FormData(e.currentTarget).get('value') ?? '')); }}>
                      {/* key: reset the field when the stored value changes */}
                      <input key={m.value} name="value" defaultValue={m.value} aria-label={`Value of ${m.key}`} title="Edit, then press Enter to save" />
                    </form>
                  ) : m.value}
                </td>
                <td>{m.source}</td>
                <td>{ago(m.saved_ms)}</td>
                <td>{m.last_used_ms ? ago(m.last_used_ms) : '—'}</td>
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
