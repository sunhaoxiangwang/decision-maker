export class BodyLimitError extends Error {}
export class BodyTimeoutError extends Error {}

/** Enforce bytes before buffering/JSON parsing, for chunked and fixed-length bodies. */
export async function readBoundedJson(message: Request | Response, maxBytes: number, timeoutMs = 20_000): Promise<unknown> {
  const length = Number(message.headers.get('content-length'));
  if (Number.isFinite(length) && length > maxBytes) throw new BodyLimitError('Body exceeds the byte limit.');
  if (!message.body) throw new SyntaxError('Missing JSON body.');
  const reader = message.body.getReader();
  const decoder = new TextDecoder();
  let bytes = 0;
  let text = '';
  let timer: ReturnType<typeof setTimeout>;
  const deadline = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new BodyTimeoutError('Body read deadline exceeded.')), timeoutMs);
  });
  try {
    while (true) {
      const chunk = await Promise.race([reader.read(), deadline]);
      if (chunk.done) break;
      bytes += chunk.value.byteLength;
      if (bytes > maxBytes) {
        void reader.cancel().catch(() => {});
        throw new BodyLimitError('Body exceeds the byte limit.');
      }
      text += decoder.decode(chunk.value, { stream: true });
    }
    return JSON.parse(text + decoder.decode());
  } catch (e) {
    void reader.cancel().catch(() => {});
    throw e;
  } finally {
    clearTimeout(timer!);
    reader.releaseLock();
  }
}
