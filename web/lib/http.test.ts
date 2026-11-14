import assert from 'node:assert/strict';
import test from 'node:test';
import { BodyLimitError, readBoundedJson } from './http.ts';

test('JSON bytes are limited before parsing, including chunked multibyte text', async () => {
  const encoder = new TextEncoder();
  const body = new ReadableStream({ start(controller) {
    controller.enqueue(encoder.encode('{"text":"'));
    controller.enqueue(encoder.encode('波'.repeat(100)));
    controller.enqueue(encoder.encode('"}'));
    controller.close();
  } });
  await assert.rejects(readBoundedJson(new Response(body), 100), BodyLimitError);
  assert.deepEqual(await readBoundedJson(new Response('{"ok":true}'), 100), { ok: true });
  await assert.rejects(readBoundedJson(new Response('{}', { headers: { 'content-length': '1000' } }), 100), BodyLimitError);
});


test('stalled body reads have a deadline and release their reader', async () => {
  await assert.rejects(readBoundedJson(new Response(new ReadableStream({})), 100, 10), /deadline/);
});

test('extraction concurrency is bounded and its output has an explicit token budget', async () => {
  const { POST } = await import('../app/llm/memory/route.ts');
  const savedFetch = globalThis.fetch;
  const savedKey = process.env.OPENAI_API_KEY;
  process.env.OPENAI_API_KEY = 'offline-test';
  const pending: ((response: Response) => void)[] = [];
  globalThis.fetch = async (_url, options) => {
    assert.equal(JSON.parse(String(options?.body)).max_output_tokens, 2048);
    assert.ok(options?.signal);
    return new Promise<Response>((resolve) => pending.push(resolve));
  };
  const request = () => new Request('http://localhost/llm/memory', { method: 'POST', body: JSON.stringify({ source: 'user', text: 'Budget $1,200.' }) });
  try {
    const first = POST(request());
    const second = POST(request());
    assert.equal((await POST(request())).status, 503);
    await new Promise(setImmediate);
    assert.equal(pending.length, 2);
    pending.forEach((resolve) => resolve(Response.json({ output: [{ type: 'message', content: [{ type: 'output_text', text: '{"memories":[]}' }] }] })));
    assert.deepEqual((await Promise.all([first, second])).map((r) => r.status), [200, 200]);
  } finally {
    globalThis.fetch = savedFetch;
    if (savedKey === undefined) delete process.env.OPENAI_API_KEY;
    else process.env.OPENAI_API_KEY = savedKey;
  }
});
