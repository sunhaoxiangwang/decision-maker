// Run: npm test
import assert from 'node:assert/strict';
import test from 'node:test';
import { asPrompt, decisionRequest, retrieve, words } from './memory.ts';
import type { MemoryItem } from './store.ts';

const note = (id: string, kind: MemoryItem['kind'], subject: string, text: string, keywords: string[], extra: Partial<MemoryItem> = {}): MemoryItem => ({
  id, kind, subject, text, keywords, status: 'confirmed', source: { type: 'conversation', conversation_id: 'c0', turn_ids: [3] }, saved_ms: 1, ...extra,
});

const memory = [
  note('budget', 'constraint', 'user', 'The budget is at most $1,200 including delivery.', ['budget', '1200', 'delivery']),
  note('return', 'preference', 'user', 'The user wants at least a 100-night return.', ['return', '100', 'nights']),
  note('seller', 'commitment', 'Seller', 'The seller offered free delivery last week.', ['delivery', 'free']),
  note('lunch', 'preference', 'user', 'The user dislikes spicy food.', ['spicy', 'food']),
  note('maybe', 'fact', 'Seller', 'The seller mentioned a return policy.', ['return'], { status: 'proposed' }),
];

test('words: numbers lose $ and commas, small words go', () => {
  assert.deepEqual(words('I can do $1,150 with a 30-day return'), ['1150', '30', 'day', 'return']);
});

test('retrieve: matching notes first, constraints always, proposals never', () => {
  const ids = retrieve(memory, 'What about the return policy and delivery?', { subject: 'Seller' }).map((m) => m.id);
  assert.deepEqual(ids, ['seller', 'budget', 'return']); // seller: delivery + subject; budget: delivery; return: return
  assert.deepEqual(retrieve(memory, 'hello there').map((m) => m.id), ['budget']); // only the constraint
});

test('mandatory constraints and user context cannot be displaced by optional top-k', () => {
  const constraints = Array.from({ length: 9 }, (_, i) => note(`limit-${i}`, 'constraint', 'user', `Required limit ${i}`, []));
  const context = note('context', 'context', 'user', 'Budget is $1,200.', [], { source: { type: 'user' } });
  const optional = Array.from({ length: 20 }, (_, i) => note(`offer-${i}`, 'fact', 'Seller', 'Price offer', ['price']));
  const chosen = retrieve([...constraints, context, ...optional], 'price', { k: 2 });
  assert.equal(chosen.length, 12);
  assert.ok([...constraints, context].every((m) => chosen.includes(m)));
  assert.equal(retrieve([...constraints, context, ...optional], 'price', { k: 0 }).length, 10);
});

test('decisionRequest: the turn, the turns before it, and its memory', () => {
  const conv = { id: 'c1', started_ms: 1, mode: 'negotiation' as const, turns: [
    { turn_id: 1, speaker: 0, text: 'Hi', time_ms: 1 },
    { turn_id: 2, speaker: 1, text: 'It is $1,150 with delivery', time_ms: 2 },
  ] };
  const req = decisionRequest(conv, 2, memory, (s) => (s === 1 ? 'Seller' : 'Buyer'))!;
  assert.deepEqual(req.turn, { turn_id: 2, speaker: 'Seller', text: 'It is $1,150 with delivery' });
  assert.deepEqual(req.recent.map((t) => t.speaker), ['Buyer']);
  assert.deepEqual(req.memory.map((m) => m.id), ['seller', 'budget']);
  assert.equal(req.memory[1].source, 'conversation c0, turns 3');
  assert.match(asPrompt(memory.slice(0, 1)), /^- \[constraint\] user: The budget is at most \$1,200/);
});
