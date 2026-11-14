// Run: npm test
import assert from 'node:assert/strict';
import test from 'node:test';
import { negotiationDemo } from './fixtures.ts';
import { initial, reducer, selectedTurn, type Action } from './store.ts';

const run = (actions: Action[], from = initial) => actions.reduce(reducer, from);

test('negotiation demo gives the expected cues', () => {
  const s = run(negotiationDemo(1_000_000).events);
  assert.deepEqual([1, 3, 4, 5].map((id) => s.turns[id].cue), ['Pause', 'Pause', 'Ask', 'Proceed']);
  assert.equal(s.turns[2].shown, false); // late cue: not sent to the band
  assert.equal(s.turns[4].decisionId, 'd4');
  assert.deepEqual(s.decisions.d4.missing, ['return_nights']);
  assert.equal(s.turns[6].final, false);
  assert.equal(selectedTurn(s), 5);
  assert.equal(Object.keys(s.memory).length, 3);
});

test('final replaces interim, and a stray interim never replaces final', () => {
  const say = (text: string, final: boolean): Action => ({ type: 'transcript', turn_id: 1, speaker: 0, text, final, time_ms: 1 });
  let s = run([say('one thou', false), say('$1,150.', true)]);
  assert.equal(s.turns[1].text, '$1,150.');
  s = reducer(s, say('one', false));
  assert.equal(s.turns[1].text, '$1,150.');
});

test('a pinned turn stays selected; unpinning follows the latest', () => {
  let s = run([{ type: 'pin', turn_id: 1 }, { type: 'cue', turn_id: 7, cue: 'Ask', shown: true, time_ms: 1 }]);
  assert.equal(selectedTurn(s), 1);
  assert.equal(s.turns[7].text, ''); // cue for a turn never seen spoken
  s = reducer(s, { type: 'pin', turn_id: null });
  assert.equal(selectedTurn(s), 7);
});

test('memory delete removes; hydrate fills gaps without overwriting live data', () => {
  const item = { id: 'm1', key: 'k', value: 'live', status: 'confirmed' as const, source: 'user', saved_ms: 1, used_by: 0 };
  let s = run([{ type: 'memory', op: 'confirmed', item }]);
  s = reducer(s, { type: 'hydrate', memory: [{ ...item, value: 'stale' }, { ...item, id: 'm2' }] });
  assert.equal(s.memory.m1.value, 'live');
  assert.ok(s.memory.m2);
  s = reducer(s, { type: 'memory', op: 'deleted', item });
  assert.equal(s.memory.m1, undefined);
});
