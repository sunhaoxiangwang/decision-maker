// Run: npm test
import assert from 'node:assert/strict';
import test from 'node:test';
import { initial, reducer, selectedTurn, visibleTurns, type Action, type DecisionEvent } from './store.ts';

const run = (actions: Action[], from = initial) => actions.reduce(reducer, from);

const decision = (id: string, turn_id: number, cue: DecisionEvent['cue']): DecisionEvent => ({
  type: 'decision', id, turn_id, mode: 'negotiation', cue, reason: 'r', facts: [], missing: ['return_nights'], conflicts: [],
  memory_used: [], latency_ms: { transcript_to_decision: 1 }, time_ms: 1,
});

test('a decision joins its turn, keeps the turns before it, and remembers a late cue', () => {
  const say = (turn_id: number, text: string): Action => ({ type: 'transcript', turn_id, speaker: 0, text, final: true, time_ms: 1 });
  const s = run([say(1, '$1,399'), say(2, 'too high'), decision('d2', 2, 'Pause'), { type: 'cue', turn_id: 2, cue: 'Pause', shown: false, time_ms: 2 }]);
  assert.equal(s.turns[2].decisionId, 'd2');
  assert.equal(s.turns[2].cue, 'Pause');
  assert.deepEqual(s.decisions.d2.context?.map((t) => t.text), ['$1,399', 'too high']);
  assert.equal(s.decisions.d2.shown, false);
  assert.equal(selectedTurn(s), 2);
});

test('saved decisions and memory load without overwriting what arrived live or attaching to new turns', () => {
  const old = { ...decision('old', 1, 'Ask'), reason: 'saved' };
  let s = run([{ type: 'transcript', turn_id: 1, speaker: 0, text: 'hi', final: true, time_ms: 1 }, decision('live', 1, 'Proceed')]);
  s = reducer(s, { type: 'load', saved: { decisions: { old, live: { ...old, id: 'live' } } } });
  assert.ok(s.loaded);
  assert.equal(s.decisions.old.reason, 'saved');
  assert.equal(s.decisions.live.cue, 'Proceed'); // live wins
  assert.equal(s.turns[1].decisionId, 'live');    // a saved decision's turn 1 is not this turn 1
  s = reducer(s, { type: 'forget-decisions' });
  assert.deepEqual([Object.keys(s.decisions).length, s.turns[1].decisionId], [0, undefined]);
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

test('memory events add and delete items', () => {
  const item = { id: 'm1', kind: 'fact' as const, subject: 'Seller', text: 'Delivery costs $50.', keywords: ['delivery'], status: 'proposed' as const, source: { type: 'user' as const }, saved_ms: 1 };
  let s = run([{ type: 'memory', op: 'proposed', item }, { type: 'memory', op: 'confirmed', item: { ...item, status: 'confirmed' } }]);
  assert.equal(s.memory.m1.status, 'confirmed');
  s = reducer(s, { type: 'memory', op: 'deleted', item });
  assert.equal(s.memory.m1, undefined);
});

test('final turns are kept in a conversation; a bridge restart or New conversation starts another', () => {
  const say = (turn_id: number, text: string, final = true, speaker = 0): Action => ({ type: 'transcript', turn_id, speaker, text, final, time_ms: turn_id });
  let s = run([say(1, 'hi', false), say(1, 'hi'), say(2, '', true), say(2, 'I can do $1,150', true, 1)]);
  const first = s.conversations[s.current!];
  assert.deepEqual(first.turns.map((t) => [t.turn_id, t.speaker, t.text]), [[1, 0, 'hi'], [2, 1, 'I can do $1,150']]); // no interim, no empty
  s = reducer(s, say(1, 'new run')); // the bridge counts from 1 again
  assert.notEqual(s.current, first.id);
  assert.equal(Object.keys(s.conversations).length, 2);
  assert.deepEqual(Object.keys(s.turns), ['1']);                  // old run's turns left the screen
  s = run([{ type: 'new-conversation' }, say(2, 'after the button')], s);
  assert.equal(Object.keys(s.conversations).length, 3);
});

test('old key/value memory is dropped on load, conversations are restored', () => {
  const conv = { id: 'c1', started_ms: 1, mode: 'negotiation' as const, turns: [] };
  const s = reducer(initial, { type: 'load', saved: { memory: { old: { id: 'old', key: 'k', value: 'v' } } as never, conversations: { c1: conv }, current: 'c1' } });
  assert.deepEqual([Object.keys(s.memory), s.current, s.conversations.c1.id], [[], 'c1', 'c1']);
});

test('empty interim guesses (silence) draw no turn', () => {
  const say = (turn_id: number, text: string, final: boolean): Action => ({ type: 'transcript', turn_id, speaker: 0, text, final, time_ms: 1 });
  let s = run([say(1, '', false)]);
  assert.equal(visibleTurns(s).length, 0);
  s = run([say(1, 'hello', true), say(2, '', false)], s);
  assert.deepEqual(visibleTurns(s).map((t) => t.turn_id), [1]);
});

test('wave → spoken context is held for confirmation, never joins the conversation, then replaces user memory', () => {
  const say = (turn_id: number, text: string, final: boolean, purpose?: 'context'): Action => ({ type: 'transcript', turn_id, speaker: 0, text, final, time_ms: turn_id, purpose });
  const mine = { id: 'old', kind: 'context' as const, subject: 'user', text: 'Budget $1,200.', keywords: [], status: 'confirmed' as const, source: { type: 'user' as const }, saved_ms: 1 };
  const heard = { ...mine, id: 'heard', source: { type: 'conversation' as const, conversation_id: 'c', turn_ids: [1] } };
  let s = run([
    { type: 'memory', op: 'confirmed', item: mine }, { type: 'memory', op: 'confirmed', item: heard },
    say(1, 'hello', true), { type: 'gesture', name: 'wave', time_ms: 2 },
    say(2, 'budget is now', false, 'context'), say(2, 'Budget is now $1,300', true, 'context'), say(3, 'and white glove delivery', true, 'context'),
  ]);
  assert.deepEqual([s.context?.listening, s.context?.text, s.context?.turn_ids], [false, 'Budget is now $1,300 and white glove delivery', [2, 3]]);
  assert.deepEqual(visibleTurns(s).map((t) => t.text), ['hello']);
  assert.equal(s.conversations[s.current!].turns.length, 1);
  s = run([{ type: 'replace-user-memory', items: [{ ...mine, id: 'new', text: 'Budget is now $1,300.' }] }, { type: 'context-done' }], s);
  assert.deepEqual(Object.keys(s.memory).sort(), ['heard', 'new']); // typed context replaced, heard memory kept
  assert.equal(s.context, null);
});

test('triple tap: on opens a new conversation and clears the screen; off closes it; an empty one is dropped', () => {
  const say = (turn_id: number, text: string): Action => ({ type: 'transcript', turn_id, speaker: 0, text, final: true, time_ms: 10 + turn_id });
  const mode = (decisions: boolean, conversation: number, time_ms: number): Action => ({ type: 'mode', decisions, conversation, time_ms });
  let s = run([mode(true, 1, 1), say(1, 'hello'), say(2, 'I can do $1,150')]);
  const first = s.current!;
  assert.equal(s.decisionsOn, true);
  assert.deepEqual(s.conversations[first].turns.map((t) => t.text), ['hello', 'I can do $1,150']);
  s = run([mode(false, 1, 20)], s);
  assert.deepEqual([s.decisionsOn, s.current], [false, null]);
  s = run([mode(true, 2, 30), say(3, 'second talk')], s);
  assert.notEqual(s.current, first);
  assert.deepEqual(visibleTurns(s).map((t) => t.text), ['second talk']);  // screen cleared
  s = run([mode(true, 2, 30)], s);                                          // replayed on reconnect: no new conversation
  assert.equal(s.conversations[s.current!].turns.length, 1);
  s = run([mode(false, 2, 40), mode(true, 3, 50), mode(false, 3, 60)], s); // nobody spoke in 3
  assert.equal(Object.keys(s.conversations).length, 2);
});
