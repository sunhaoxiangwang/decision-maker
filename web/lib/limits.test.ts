import assert from 'node:assert/strict';
import test from 'node:test';
import { byteSize, MAX_HISTORY_BYTES, MAX_TURNS } from './limits.ts';
import { initial, reducer } from './store.ts';

test('long sessions retain bounded turns/bytes and disclose pruning', () => {
  let state = reducer(initial, { type: 'mode', decisions: true, conversation: 1, time_ms: 1 });
  for (let turn_id = 1; turn_id <= 250; turn_id++) {
    state = reducer(state, { type: 'transcript', turn_id, speaker: 0, text: 'x'.repeat(8000), final: true, time_ms: turn_id });
  }
  assert.ok(Object.keys(state.turns).length <= MAX_TURNS);
  assert.ok(state.conversations[state.current!].turns.length <= MAX_TURNS);
  assert.ok(byteSize({ conversations: state.conversations, decisions: state.decisions }) <= MAX_HISTORY_BYTES);
  assert.match(state.notice!, /retention limit/);
  assert.equal(state.conversations[state.current!].turns.at(-1)?.turn_id, 250);
});

test('memory limits reject the new item visibly and retain existing mandatory constraints', () => {
  const item = { id: 'budget', kind: 'constraint' as const, subject: 'user', text: 'Budget $1,200.', keywords: [], status: 'confirmed' as const, source: { type: 'user' as const }, saved_ms: 1 };
  let state = reducer(initial, { type: 'memory', op: 'confirmed', item });
  state = reducer(state, { type: 'memory', op: 'confirmed', item: { ...item, id: 'too-large', text: '波'.repeat(20_000) } });
  assert.deepEqual(Object.keys(state.memory), ['budget']);
  assert.equal(state.memory.budget, item);
  assert.match(state.notice!, /not saved/);
  state = reducer(state, { type: 'gap', message: 'Replay expired; history may be incomplete.' });
  assert.match(state.notice!, /incomplete/);
});
