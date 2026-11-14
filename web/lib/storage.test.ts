import assert from 'node:assert/strict';
import test from 'node:test';
import { readSaved, writeSaved } from './storage.ts';

test('quota failures are reported, successful saves clear the error, malformed copies are not overwritten', () => {
  assert.match(writeSaved({ setItem() { throw new Error('QuotaExceededError'); } }, {})!, /not being saved/);
  let raw = '';
  assert.equal(writeSaved({ setItem(_key, value) { raw = value; } }, { current: null }), null);
  assert.deepEqual(readSaved({ getItem() { return raw; } }), { current: null });
  const broken = { getItem() { return '{broken'; } };
  assert.throws(() => readSaved(broken), SyntaxError);
  assert.equal(broken.getItem(), '{broken');
  assert.throws(() => readSaved({ getItem() { return 'x'.repeat(2 * 1024 * 1024 + 1); } }), /loading limit/);
});
