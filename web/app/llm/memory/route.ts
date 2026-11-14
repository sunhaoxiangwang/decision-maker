// Turns a past conversation, or context the user typed, into memory notes (OpenAI).
// Runs on the Next server: OPENAI_API_KEY is read from the environment (or web/.env.local)
// and never reaches the browser. proxy.ts only lets this UI's own JSON POSTs through.
//
// POST { source: 'conversation', conversation_id, transcript: "t3 Seller: …\n…" }
//    | { source: 'user', text: "…" }
// →    { memories: [{ kind, subject, text, keywords, turn_ids }] }

import { BodyLimitError, BodyTimeoutError, readBoundedJson } from '../../../lib/http.ts';

const MODEL = process.env.OPENAI_MODEL ?? 'gpt-6-luna'; // the kit's default model
const MAX_CHARS = 60_000;
const MAX_BODY_BYTES = 64 * 1024;
let activeRequests = 0;

const SCHEMA = {
  type: 'object', additionalProperties: false, required: ['memories'],
  properties: {
    memories: {
      type: 'array',
      items: {
        type: 'object', additionalProperties: false,
        required: ['kind', 'subject', 'text', 'keywords', 'turn_ids'],
        properties: {
          kind: { type: 'string', enum: ['constraint', 'preference', 'commitment', 'fact', 'context'] },
          subject: { type: 'string' },
          text: { type: 'string' },
          keywords: { type: 'array', items: { type: 'string' } },
          turn_ids: { type: 'array', items: { type: 'integer' } },
        },
      },
    },
  },
};

const INSTRUCTIONS = `You write long-term memory for an assistant that helps one user make decisions during
negotiations and everyday choices (for example buying a mattress, or picking lunch).
From the input, write memory notes that would help a later decision:
- constraint: hard limits or required terms (budgets, deadlines, minimum return periods)
- preference: likes and dislikes
- commitment: what someone agreed to or promised, with the exact terms
- fact: stated facts about an offer, a product, a place or a person
- context: background about the situation
Rules: one self-contained sentence per note that names its subject and keeps exact numbers.
subject is the speaker's name as written in the input, or "user" for the person using the
assistant. Only what the input actually says: never guess or infer. keywords: 3 to 8
lower-case single words a later turn would use (numbers without "$" or commas). turn_ids:
the t-numbers the note comes from; empty for typed context. No notes for small talk.
An empty list is a valid answer.`;

const bad = (status: number, error: string) => Response.json({ error }, { status });

export async function POST(req: Request) {
  if (activeRequests >= 2) return bad(503, 'Memory extraction is busy. Try again shortly.');
  activeRequests++;
  try { return await extract(req); }
  finally { activeRequests--; }
}

async function extract(req: Request) {
  const key = process.env.OPENAI_API_KEY;
  if (!key) return bad(503, 'No OpenAI key on the server: set OPENAI_API_KEY in web/.env.local and restart npm run dev.');

  let body: any;
  try { body = await readBoundedJson(req, MAX_BODY_BYTES); }
  catch (e) { return bad(e instanceof BodyLimitError ? 413 : e instanceof BodyTimeoutError ? 408 : 400, 'Invalid, oversized or timed-out JSON body.'); }
  let input: string;
  if (body?.source === 'conversation' && typeof body.transcript === 'string' && typeof body.conversation_id === 'string') {
    input = `A conversation (one line per turn: t<number> <speaker>: <words>):\n${body.transcript}`;
  } else if (body?.source === 'user' && typeof body.text === 'string') {
    input = `Context the user typed about themselves and this negotiation:\n${body.text}`;
  } else {
    return bad(400, 'Send { source: "conversation", conversation_id, transcript } or { source: "user", text }.');
  }
  if (input.length > MAX_CHARS) return bad(413, `Too long to summarise at once (over ${MAX_CHARS} characters).`);

  const r = await fetch('https://api.openai.com/v1/responses', {
    method: 'POST',
    signal: AbortSignal.any([req.signal, AbortSignal.timeout(20_000)]),
    headers: { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({
      model: MODEL, max_output_tokens: 2048, instructions: INSTRUCTIONS, input,
      text: { format: { type: 'json_schema', name: 'memories', schema: SCHEMA, strict: true } },
    }),
  }).catch(() => null);
  // Only the status number is passed on, never what OpenAI wrote (it can echo the request).
  if (!r) return bad(502, 'Could not reach api.openai.com.');
  if (!r.ok) return bad(502, `OpenAI answered status ${r.status}.`);

  let out: any;
  try { out = await readBoundedJson(r, MAX_BODY_BYTES); }
  catch { return bad(502, 'The model response was invalid or exceeded the byte/time limit.'); }
  const text: string | undefined = out?.output
    ?.flatMap((o: { type: string; content?: { type: string; text?: string }[] }) => (o.type === 'message' ? o.content ?? [] : []))
    .find((c: { type: string }) => c.type === 'output_text')?.text;
  try {
    const { memories } = JSON.parse(text ?? '');
    return Response.json({ memories: Array.isArray(memories) ? memories : [] });
  } catch {
    return bad(502, 'The model did not answer in the expected shape.');
  }
}
